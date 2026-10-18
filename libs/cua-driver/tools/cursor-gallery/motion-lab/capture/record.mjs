// Deterministic capture of the motion lab through Chrome DevTools Protocol.
// The page runs with ?capture=1, so time only advances when we call
// window.__lab.setTime(ms); every frame is exact regardless of machine speed.
//
//   node capture/record.mjs shot  URL OUT.png  [--w 1920 --h 1080 --t 1200]
//   node capture/record.mjs video URL OUT_DIR  [--w 1920 --h 1080 --fps 30 --seconds 12]
//
// Needs CDP_ENDPOINT (default http://127.0.0.1:9333) of a running headless
// Chrome. Video mode writes OUT_DIR/%05d.jpg for ffmpeg.

import fs from 'node:fs/promises';
import path from 'node:path';

const [mode, url, out, ...rest] = process.argv.slice(2);
const opt = { w: 1920, h: 1080, t: 1500, fps: 30, seconds: 12, scale: 1 };
for (let i = 0; i < rest.length; i += 2) opt[rest[i].replace(/^--/, '')] = Number(rest[i + 1]);
if (!mode || !url || !out) {
  console.error('usage: record.mjs shot|video URL OUT [--w --h --t --fps --seconds --scale]');
  process.exit(2);
}

const endpoint = process.env.CDP_ENDPOINT ?? 'http://127.0.0.1:9333';
const target = await fetch(`${endpoint}/json/new?about:blank`, { method: 'PUT' }).then((r) =>
  r.json()
);
const socket = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true });
  socket.addEventListener('error', reject, { once: true });
});
let nextId = 1;
const pending = new Map();
socket.addEventListener('message', (event) => {
  const msg = JSON.parse(event.data);
  const w = pending.get(msg.id);
  if (!w) return;
  pending.delete(msg.id);
  if (msg.error) w.reject(new Error(JSON.stringify(msg.error)));
  else w.resolve(msg.result);
});
const send = (method, params = {}) =>
  new Promise((resolve, reject) => {
    const id = nextId++;
    pending.set(id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params }));
  });
const evaluate = async (expression) => {
  const r = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails)
    throw new Error(r.exceptionDetails.exception?.description ?? r.exceptionDetails.text);
  return r.result.value;
};

await send('Page.enable');
await send('Runtime.enable');
await send('Emulation.setDeviceMetricsOverride', {
  width: opt.w,
  height: opt.h,
  deviceScaleFactor: opt.scale,
  mobile: false,
});
await send('Page.navigate', { url });
await evaluate(`new Promise((res, rej) => { const d = Date.now() + 20000; (function poll() {
  const ready = document.documentElement.dataset.labReady === '1';
  if (ready && (document.fonts.status === 'loaded' || Date.now() > d - 12000)) res(true);
  else if (Date.now() > d) rej(new Error('lab not ready')); else setTimeout(poll, 50); })(); })`);
await new Promise((r) => setTimeout(r, 300));

const shot = async (file, format = 'png') => {
  const r = await send('Page.captureScreenshot', {
    format,
    quality: format === 'jpeg' ? 92 : undefined,
    captureBeyondViewport: false,
  });
  await fs.writeFile(file, Buffer.from(r.data, 'base64'));
};

if (mode === 'shot') {
  await evaluate(`window.__lab.setTime(${opt.t})`);
  await new Promise((r) => setTimeout(r, 400));
  await shot(out, 'png');
} else if (mode === 'video') {
  await fs.mkdir(out, { recursive: true });
  const n = Math.round(opt.fps * opt.seconds);
  for (let i = 0; i < n; i++) {
    await evaluate(`window.__lab.setTime(${(i * 1000) / opt.fps})`);
    await shot(path.join(out, `${String(i).padStart(5, '0')}.jpg`), 'jpeg');
  }
}
await fetch(`${endpoint}/json/close/${target.id}`);
socket.close();
console.log(`record: ${mode} -> ${out}`);
