const readline = require('node:readline');
const { Worker } = require('node:worker_threads');

const WORKER_SOURCE = String.raw`
const { parentPort, workerData } = require('node:worker_threads');
const { stripTypeScriptTypes } = require('node:module');
const { format } = require('node:util');

// Defense in depth over the process permission boundary: model code receives
// declared bindings, not ambient Node handles.
for (const name of ['process', 'require', 'module', 'Buffer']) {
  try { delete globalThis[name]; } catch {}
}

const pending = new Map();
let nextId = 1;
let workerPhase = 'preparing';
let injected;
const logs = [];
const consoleShim = Object.freeze({ log: (...args) => logs.push(format(...args)) });

function protocolFailure(message) {
  workerPhase = 'failed';
  parentPort.postMessage({ type: 'protocol_failure', message });
}

parentPort.on('message', (message) => {
  if (message?.type === 'run') {
    if (workerPhase !== 'ready' || typeof message.program !== 'string') {
      protocolFailure('expected exactly one dispatched program');
      return;
    }
    workerPhase = 'running';
    execute(message.program).catch((error) => protocolFailure(String(error?.message ?? error)));
    return;
  }
  if (message?.type !== 'binding_result' || workerPhase !== 'running') {
    protocolFailure('unexpected worker input before or outside program dispatch');
    return;
  }
  const entry = pending.get(message.id);
  if (!entry) return;
  pending.delete(message.id);
  if (message.ok) entry.resolve(message.value);
  else {
    const error = entry.typedError
      ? new entry.ErrorClass(entry.name, message.message)
      : new Error(message.message);
    entry.reject(error);
  }
});

function errorClass(descriptor) {
  return class BindingCallError extends Error {
    constructor(member, message) {
      super(message);
      this.name = descriptor.name;
      this[descriptor.member_name_property] = member;
    }
  };
}

function bindings(data) {
  const globals = [];
  const values = [];
  const errors = [];
  const errorValues = [];
  for (const namespace of data) {
    const root = Object.create(null);
    const ErrorClass = namespace.error_class ? errorClass(namespace.error_class) : Error;
    for (const name of namespace.names) {
      Object.defineProperty(root, name, {
        enumerable: true,
        value: (args) => new Promise((resolve, reject) => {
          const id = nextId++;
          pending.set(id, { resolve, reject, ErrorClass, name, typedError: !!namespace.error_class });
          parentPort.postMessage({
            type: 'binding_call', id, global: namespace.global, name, args,
          });
        }),
      });
    }
    globals.push(namespace.global);
    values.push(root);
    if (namespace.error_class) {
      errors.push(namespace.error_class.name);
      errorValues.push(ErrorClass);
    }
  }
  return { globals, values, errors, errorValues };
}

class InvalidOutputError extends Error {
  constructor(message) {
    super(message);
    this.name = 'InvalidOutputError';
  }
}

function cloneJson(value, path = '$', ancestors = new Set()) {
  if (value === null || typeof value === 'string' || typeof value === 'boolean') {
    return value;
  }
  if (typeof value === 'number') {
    if (!Number.isFinite(value) || Object.is(value, -0)) {
      throw new InvalidOutputError(path + ' is not a lossless JSON number');
    }
    return value;
  }
  if (typeof value !== 'object') {
    throw new InvalidOutputError(path + ' has non-JSON type ' + typeof value);
  }
  if (ancestors.has(value)) {
    throw new InvalidOutputError(path + ' is cyclic');
  }

  ancestors.add(value);
  try {
    if (Array.isArray(value)) {
      const result = [];
      for (let index = 0; index < value.length; index += 1) {
        if (!Object.hasOwn(value, index)) {
          throw new InvalidOutputError(path + '[' + index + '] is an array hole');
        }
        result.push(cloneJson(value[index], path + '[' + index + ']', ancestors));
      }
      if (Reflect.ownKeys(value).some((key) => key !== 'length' && !/^0$|^[1-9]\d*$/.test(String(key)))) {
        throw new InvalidOutputError(path + ' has a non-index array property');
      }
      return result;
    }

    const prototype = Object.getPrototypeOf(value);
    if (prototype !== Object.prototype && prototype !== null) {
      throw new InvalidOutputError(path + ' is not a plain JSON object');
    }
    const result = {};
    for (const key of Reflect.ownKeys(value)) {
      if (typeof key !== 'string') {
        throw new InvalidOutputError(path + ' has a symbol key');
      }
      const descriptor = Object.getOwnPropertyDescriptor(value, key);
      if (!descriptor?.enumerable || !('value' in descriptor)) {
        throw new InvalidOutputError(path + '.' + key + ' is not an enumerable data property');
      }
      result[key] = cloneJson(descriptor.value, path + '.' + key, ancestors);
    }
    return result;
  } finally {
    ancestors.delete(value);
  }
}

function lossless(value) {
  if (value === undefined) return { has_value: false };
  return { has_value: true, value: cloneJson(value) };
}

async function execute(program) {
  try {
    const prefix = 'async function __dsh_program__() {\n';
    const suffix = '\n}';
    const wrapped = stripTypeScriptTypes(prefix + program + suffix, { mode: 'strip' });
    const code = wrapped.slice(prefix.length, wrapped.length - suffix.length);
    const AsyncFunction = Object.getPrototypeOf(async function () {}).constructor;
    const fn = new AsyncFunction(
      ...injected.globals,
      ...injected.errors,
      'console',
      '\"use strict\";\n' + code,
    );
    const value = await fn(...injected.values, ...injected.errorValues, consoleShim);
    workerPhase = 'complete';
    parentPort.postMessage({ type: 'complete', ...lossless(value), logs, error: null });
  } catch (error) {
    workerPhase = 'complete';
    parentPort.postMessage({
      type: 'complete', has_value: false, logs,
      error: {
        kind: error instanceof InvalidOutputError ? 'invalid-output' : 'exception',
        message: String(error?.stack ?? error?.message ?? error),
      },
    });
  }
}

try {
  injected = bindings(workerData.namespaces);
  // Initialize the trusted TypeScript stripper without receiving or parsing
  // model code before the Host's dispatch guard permits execution.
  stripTypeScriptTypes('const __dsh_runtime_warmup__: number = 0;', { mode: 'strip' });
  workerPhase = 'ready';
  parentPort.postMessage({ type: 'ready' });
} catch (error) {
  workerPhase = 'failed';
  parentPort.postMessage({
    type: 'worker_failure', message: String(error?.message ?? error),
  });
}
`;

let worker;
let settled = false;
let computeTimer;
let wallTimer;
let phase = 'unprepared';
let computeBaseline;

function updateWallBudget(paused, remaining) {
  clearTimeout(wallTimer);
  if (!paused) wallTimer = setTimeout(() => finish({
    type: 'complete', has_value: false, logs: [],
    error: { kind: 'timeout', message: 'wall-clock ceiling reached' },
  }), remaining);
}

function send(message, done) {
  process.stdout.write(JSON.stringify(message) + '\n', done);
}

function resultBytes(message) {
  const payload = {
    value: message.has_value ? message.value : null,
    logs: message.logs,
    error: message.error,
  };
  return Buffer.byteLength(JSON.stringify(payload), 'utf8');
}

function boundedCompletion(message) {
  if (message.type !== 'complete' || resultBytes(message) <= limits.max_output_bytes) {
    return message;
  }
  return {
    type: 'complete', has_value: false, logs: [],
    error: { kind: 'output-limit', message: 'max_output_bytes exceeded' },
  };
}

function finish(message) {
  if (settled) return;
  settled = true;
  clearInterval(computeTimer);
  clearTimeout(wallTimer);
  const complete = () => send(boundedCompletion(message), () => process.exit(0));
  if (worker) worker.terminate().then(complete, complete);
  else complete();
}

let limits;
const input = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
input.on('line', (line) => {
  if (settled) return;
  if (!line.trim()) return;
  let message;
  try { message = JSON.parse(line); }
  catch (error) { finish({ type: 'protocol_failure', message: String(error) }); return; }
  if (!message || typeof message !== 'object' || Array.isArray(message)) {
    finish({ type: 'protocol_failure', message: 'expected a protocol object' });
    return;
  }
  // Only the trusted Host stdin controls approval waits. Worker messages are
  // handled separately and cannot pause the budget; CPU limits stay active.
  if (message.type === 'approval_state') {
    if (phase !== 'running' || typeof message.paused !== 'boolean' || !Number.isSafeInteger(message.remaining_ms) || message.remaining_ms < 0 || message.remaining_ms > limits.max_wall_ms) {
      finish({ type: 'protocol_failure', message: 'invalid approval budget state' }); return;
    }
    if (!settled) updateWallBudget(message.paused, message.remaining_ms);
    return;
  }
  if (message.type === 'binding_result') {
    if (phase !== 'running') {
      finish({ type: 'protocol_failure', message: 'binding result before program dispatch' });
      return;
    }
    worker.postMessage(message);
    return;
  }
  if (message.type === 'run') {
    if (phase !== 'ready' || typeof message.program !== 'string' || Object.keys(message).some((key) => !['type', 'program'].includes(key))) {
      finish({ type: 'protocol_failure', message: 'expected exactly one run after readiness' });
      return;
    }
    phase = 'running';
    // Sample once at dispatch. Neither approval nor binding traffic can reset
    // the baseline and hide compute performed by an unawaited busy loop.
    computeBaseline = worker.performance.eventLoopUtilization();
    updateWallBudget(false, limits.max_wall_ms);
    computeTimer = setInterval(() => {
      const utilization = worker.performance.eventLoopUtilization(computeBaseline);
      if (utilization.active > limits.compute_ms) {
        finish({
          type: 'complete', has_value: false, logs: [],
          error: { kind: 'timeout', message: 'compute budget exhausted' },
        });
      }
    }, 25);
    worker.postMessage({ type: 'run', program: message.program });
    return;
  }
  if (message.type !== 'prepare' || phase !== 'unprepared' || !Array.isArray(message.namespaces) || !message.limits || typeof message.limits !== 'object' || Object.keys(message).some((key) => !['type', 'namespaces', 'limits'].includes(key))) {
    finish({ type: 'protocol_failure', message: 'expected exactly one preparation without model code' });
    return;
  }
  phase = 'preparing';
  limits = message.limits;
  // Node and the Windows AppContainer launcher need ambient OS coordinates at
  // process startup. Erase the complete environment before the untrusted
  // Worker is created, so constructor/builtin recovery cannot observe the
  // parent HOME, PATH, proxy settings, or any other inherited value.
  for (const key of Object.keys(process.env)) delete process.env[key];
  worker = new Worker(WORKER_SOURCE, {
    eval: true,
    workerData: { namespaces: message.namespaces },
    resourceLimits: {
      maxOldGenerationSizeMb: message.limits.max_old_generation_size_mb,
    },
  });
  worker.on('message', (event) => {
    if (settled) return;
    if (event?.type === 'ready') {
      if (phase !== 'preparing') {
        finish({ type: 'protocol_failure', message: 'duplicate or unexpected worker readiness' });
        return;
      }
      phase = 'ready';
      send({ type: 'ready' });
    }
    else if (event?.type === 'complete' && phase === 'running') finish(event);
    else if (event?.type === 'binding_call' && phase === 'running') send(event);
    else if (event?.type === 'worker_failure' || event?.type === 'protocol_failure') finish(event);
    else finish({ type: 'protocol_failure', message: 'invalid worker message' });
  });
  worker.on('error', (error) => finish({ type: 'worker_failure', message: error.message }));
  worker.on('exit', (code) => {
    if (!settled) finish({ type: 'worker_failure', message: 'worker exited ' + code });
  });
});
input.on('close', () => {
  if (!settled) finish({ type: 'protocol_failure', message: 'Host stdin closed before program completion' });
});
