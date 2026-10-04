// Real loopback/process fixture for the desktop's CLI readiness handshake.
const fs = require('node:fs');
const http = require('node:http');
const crypto = require('node:crypto');

const [mode, home, fakePid, ...args] = process.argv.slice(2);
if (mode === 'exit') process.exit(23);
const option = name => args.includes(name) ? args[args.indexOf(name) + 1] : undefined;
const readyFile = option('--ready-file');
const stdioLog = option('--stdio-log');
if (mode === 'early-stderr') {
  process.stderr.write('dsh: --ready-file parent directory must already exist；路径错误中文🙂\n',
    () => process.exit(1));
  return;
}
if (mode === 'early-stdout' || mode === 'stdout-fake-home-lock') {
  process.stdout.write(mode === 'early-stdout' ? 'startup stdout-only failure 中文🙂\n' :
    '该数据目录正在使用，请先关闭其它 Harness 实例\n操作结果尚未确认\n',
    () => process.exit(1));
  return;
}
if (mode === 'unreadable-log') {
  // A directory cannot be read as a regular log file on any test platform.
  fs.mkdirSync(stdioLog);
  process.stderr.write('dsh: desktop stdio redirection failed: Access is denied. (os error 5)\n',
    () => process.exit(1));
  return;
}
if (mode === 'log-startup-error' || mode === 'log-large-tail') {
  fs.writeFileSync(stdioLog, 'dsh: desktop Host starting\n' +
    (mode === 'log-large-tail' ? 'discard-log-prefix\n' + '中文🙂'.repeat(10000) : '') +
    '\n该数据目录正在使用，请先关闭其它 Harness 实例\n');
  if (mode === 'log-startup-error') {
    process.stderr.write('additional startup stderr\n', () => process.exit(1));
    return;
  }
  process.exit(1);
}
if (mode === 'exit-large-tail' || mode === 'exit-malformed-tail') {
  const prefix = mode === 'exit-malformed-tail' ? Buffer.alloc(10000, 0xff) :
    Buffer.from('discard-stream-prefix\n' + '中文🙂'.repeat(10000));
  const last = Buffer.from('\nfinal stderr 中文🙂 after large output\n');
  process.stderr.write(prefix, () => {
    // Split one UTF-8 character across distinct writes before the exit edge.
    process.stderr.write(last.subarray(0, 15), () => setImmediate(() => {
      process.stderr.write(last.subarray(15), () => process.exit(1));
    }));
  });
  return;
}
if (mode === 'exit-open-pipes') {
  const child = require('node:child_process').spawn(process.execPath,
    ['-e', 'setTimeout(() => process.exit(0), 6000)'],
    {cwd: require('node:os').tmpdir(), stdio: ['ignore', process.stdout, process.stderr]});
  child.unref();
  process.stderr.write('original child exited; descendant keeps pipes open\n',
    () => process.exit(1));
  return;
}
if (stdioLog) fs.appendFileSync(stdioLog, 'fixture started\n');
const instanceId = crypto.randomUUID();
let reportedHome = home;
const server = http.createServer((request, response) => {
  if (stdioLog) fs.appendFileSync(stdioLog, 'fixture request after startup\n');
  if (request.url === '/__fixture/stop') {
    if (request.headers['x-dsh-fixture-instance'] !== instanceId) {
      response.writeHead(403).end();
      return;
    }
    response.end('stopping', () => server.close(() => process.exit(0)));
    return;
  }
  if (request.url === '/__fixture/home') {
    if (request.headers['x-dsh-fixture-instance'] !== instanceId) {
      response.writeHead(403).end();
      return;
    }
    let body = '';
    request.on('data', chunk => body += chunk);
    request.on('end', () => {
      reportedHome = JSON.parse(body).home;
      response.end('home changed');
    });
    return;
  }
  let body = '';
  request.on('data', chunk => body += chunk);
  request.on('end', () => {
    const message = JSON.parse(body);
    response.setHeader('Content-Type', 'application/json');
    response.end(JSON.stringify({
      type: 'server-response', rpcId: message.rpcId,
      result: {ok: true, value: {
        version: 'fixture', home: reportedHome, cwd: reportedHome,
        processId: mode === 'rpc-wrong-pid' ? process.pid + 1 : process.pid,
        instanceId,
      }},
    }));
  });
});
server.listen(Number(option('--port')), '127.0.0.1', () => {
  if (mode === 'timeout') return;
  const ready = {
    version: 1,
    pid: mode === 'wrong-pid' ? Number(fakePid) : process.pid,
    instanceId: mode === 'wrong-instance' ? crypto.randomUUID() : instanceId,
    url: mode === 'wrong-url' ? 'http://127.0.0.1:0' :
      `http://127.0.0.1:${server.address().port}`,
    executable: process.execPath,
    home,
  };
  const contents = mode === 'malformed' ? '{broken' :
    mode === 'oversized' ? ' '.repeat(4097) : JSON.stringify(ready);
  fs.writeFileSync(readyFile + '.tmp', contents, {flag: 'wx'});
  fs.renameSync(readyFile + '.tmp', readyFile);
});
