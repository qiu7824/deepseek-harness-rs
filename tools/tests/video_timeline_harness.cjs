const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const crypto = require('node:crypto').webcrypto;

const source = fs.readFileSync(path.join(__dirname, '../../crates/computer-use/tool-computer-use-command/src/browser_video.js'), 'utf8');
class Video extends EventTarget {
  constructor(duration = 10) {
    super();
    Object.assign(this, { duration, readyState: 2, videoWidth: 1280, videoHeight: 720, currentSrc: 'blob:video', textTracks: [], seeking: false, paused: false, time: 0 });
    this.listeners = new Set();
  }
  addEventListener(name, fn) { this.listeners.add(fn); super.addEventListener(name, fn); }
  removeEventListener(name, fn) { super.removeEventListener(name, fn); this.listeners.delete(fn); }
  get currentTime() { return this.time; }
  set currentTime(value) { this.time = value; queueMicrotask(() => this.dispatchEvent(new Event('seeked'))); }
  pause() { this.paused = true; }
  scrollIntoView() {}
  getBoundingClientRect() { return { left: 0, top: 0, right: 1280, bottom: 720 }; }
}
function browser(video, onFrame = () => {}, paint = true) {
  const state = { video, rafJobs: new Map() };
  let nextFrame = 0;
  const context = vm.createContext({
    HTMLVideoElement: Video, setTimeout: (fn, ms) => setTimeout(fn, Math.min(ms, 150)), clearTimeout, setInterval, clearInterval,
    crypto,
    requestAnimationFrame: fn => {
      const id = ++nextFrame;
      state.rafJobs.set(id, fn);
      if (paint) queueMicrotask(() => { if (state.rafJobs.delete(id)) { onFrame(state); fn(); } });
      return id;
    },
    cancelAnimationFrame: id => state.rafJobs.delete(id),
    scrollX: 0, scrollY: 0, innerWidth: 1280, innerHeight: 720,
    document: {
      querySelector: () => state.video,
      createElement: () => ({ getContext: () => ({ drawImage() {} }), toDataURL: () => 'data:image/png;base64,frame' })
    }
  });
  return { state, inspect: (...args) => vm.runInContext(`(${source})`, context)(...args) };
}
(async () => {
  // A real MediaSource can already have metadata while duration remains zero.
  const delayed = new Video(0), waiting = browser(delayed);
  const pending = waiting.inspect('video', 0);
  setTimeout(() => { delayed.duration = 8; delayed.dispatchEvent(new Event('durationchange')); }, 10);
  const frame = await pending;
  assert.equal(frame.duration, 8);
  assert.equal(frame.currentTime, 0);
  assert.equal(frame.captureMethod, 'decoded-frame');
  assert.equal(delayed.listeners.size, 0, 'metadata listeners released after success');

  const missing = new Video(0);
  await assert.rejects(browser(missing).inspect('video', 0), /timeline is not ready/);
  assert.equal(missing.listeners.size, 0, 'metadata listeners released after timeout');

  const finite = new Video(2);
  await assert.rejects(browser(finite).inspect('video', 2), /outside the video duration/);
  assert.equal(finite.currentTime, 0, 'explicit invalid timestamp must not seek');

  const live = new Video(Infinity); live.time = 21;
  const liveFrame = await browser(live).inspect('video', 21);
  assert.equal(liveFrame.duration, null);
  assert.equal(liveFrame.currentTime, 21);

  const replaced = new Video(0), replacement = browser(replaced);
  const old = replacement.inspect('video', null);
  replacement.state.video = new Video(4);
  await assert.rejects(old, /source changed while loading metadata/);
  assert.equal(replaced.listeners.size, 0);

  const changed = browser(new Video(), state => { state.video.currentSrc = 'blob:replacement'; });
  await assert.rejects(changed.inspect('video', 0), /source changed during frame capture/);

  const frozen = browser(new Video(), () => {}, false);
  await assert.rejects(frozen.inspect('video', 0), /frame layout timed out/);
  assert.equal(frozen.state.rafJobs.size, 0, 'timed-out layout callbacks are cancelled');

  const sameUrl = browser(new Video());
  const first = await sameUrl.inspect('video', null);
  const repeat = await sameUrl.inspect('video', null);
  assert.equal(first.elementId, repeat.elementId, 'identity survives separate evaluations');
  sameUrl.state.video = new Video();
  const second = await sameUrl.inspect('video', null);
  assert.equal(first.source, second.source);
  assert.notEqual(first.elementId, second.elementId, 'same source URL cannot hide a replacement');
  console.log('video timeline: delayed metadata, timeout cleanup, explicit bounds, live stream, source identity and layout cleanup passed');
})().catch(error => { console.error(error); process.exitCode = 1; });
