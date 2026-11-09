import fs from 'node:fs/promises';
import path from 'node:path';

const endpoint = process.env.CDP_ENDPOINT ?? 'http://127.0.0.1:9229';
const pageUrl = process.env.PAGE_URL ?? 'http://127.0.0.1:3001/';
const frameUrlRoot =
  process.env.FRAME_URL_ROOT ?? 'http://127.0.0.1:3001/target/cursor-gallery/renderer-frames';
const outputRoot = process.argv[2];
const fps = 15;
const duration = 4;

if (!outputRoot || typeof WebSocket === 'undefined') {
  throw new Error('usage: node --experimental-websocket capture-gallery.mjs OUTPUT_DIR');
}

const tabs = await fetch(`${endpoint}/json/list`).then((response) => response.json());
const tab = tabs.find((candidate) => candidate.type === 'page');
if (!tab?.webSocketDebuggerUrl) {
  throw new Error(`No page target found at ${endpoint}`);
}

const socket = new WebSocket(tab.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true });
  socket.addEventListener('error', reject, { once: true });
});

let nextId = 1;
const pending = new Map();
socket.addEventListener('message', (event) => {
  const message = JSON.parse(event.data);
  const waiter = pending.get(message.id);
  if (!waiter) return;
  pending.delete(message.id);
  if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
  else waiter.resolve(message.result);
});

function command(method, params = {}) {
  const id = nextId++;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    socket.send(JSON.stringify({ id, method, params }));
  });
}

async function evaluate(expression, awaitPromise = false) {
  const result = await command('Runtime.evaluate', {
    expression,
    awaitPromise,
    returnByValue: true,
  });
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.text);
  return result.result.value;
}

await fs.mkdir(path.join(outputRoot, 'actions'), { recursive: true });
await command('Page.enable');
await command('Runtime.enable');
await command('Emulation.setDeviceMetricsOverride', {
  width: 1600,
  height: 2200,
  deviceScaleFactor: 1,
  mobile: false,
});
await command('Page.navigate', { url: pageUrl });
await evaluate(
  `new Promise((resolve, reject) => {
    const deadline = Date.now() + 15000;
    const ready = () => {
      const videos = [...document.querySelectorAll(".cursor-video")];
      const expected = Number(document.documentElement.dataset.galleryVideoCount);
      if (Number.isFinite(expected) && videos.length === expected && videos.every((video) => video.readyState >= 2)) resolve(true);
      else if (Date.now() > deadline) reject(new Error("Timed out waiting for videos"));
      else setTimeout(ready, 50);
    };
    ready();
  })`,
  true
);

await evaluate(`(() => {
  for (const video of document.querySelectorAll("video.cursor-video")) {
    const image = document.createElement("img");
    image.className = video.className;
    image.dataset.group = video.dataset.group;
    image.dataset.state = video.dataset.state;
    image.alt = "";
    image.setAttribute("aria-hidden", "true");
    video.replaceWith(image);
  }
  return document.querySelectorAll("img.cursor-video").length;
})()`);

const clips = await evaluate(`(() => {
  const clipFor = (section, bottomPadding) => {
    const heading = section.querySelector(".section-heading").getBoundingClientRect();
    const grid = section.querySelector(".state-grid").getBoundingClientRect();
    const rect = section.getBoundingClientRect();
    return {
      x: Math.floor(rect.left - 30),
      y: Math.floor(heading.top - 28),
      width: Math.ceil(rect.width + 60),
      height: Math.ceil(grid.bottom - heading.top + 28 + bottomPadding)
    };
  };
  return {
    actions: clipFor(document.querySelector('[data-capture-group="actions"]'), 28)
  };
})()`);

for (let frame = 0; frame < fps * duration; frame += 1) {
  const sourceFrame = frame * 2;
  const loaded = await evaluate(
    `Promise.all([...document.querySelectorAll("img.cursor-video")].map((image) => {
      const src = ${JSON.stringify(frameUrlRoot)} + "/" +
        image.dataset.group + "/" + image.dataset.state + "/" +
        String(${sourceFrame}).padStart(4, "0") + ".png";
      if (image.src === src && image.complete) return Promise.resolve();
      return new Promise((resolve) => {
        const finish = () => resolve();
        image.addEventListener("load", finish, { once: true });
        image.addEventListener("error", finish, { once: true });
        image.src = src;
        setTimeout(finish, 1000);
      });
    })).then(() => [...document.querySelectorAll("img.cursor-video")]
      .every((image) => image.complete && image.naturalWidth > 0))`,
    true
  );
  if (!loaded) throw new Error(`Failed to load renderer frame ${sourceFrame}`);

  for (const [name, clip] of Object.entries(clips)) {
    const capture = await command('Page.captureScreenshot', {
      format: 'png',
      fromSurface: true,
      captureBeyondViewport: true,
      clip: { ...clip, scale: 1 },
    });
    await fs.writeFile(
      path.join(outputRoot, name, `${String(frame).padStart(4, '0')}.png`),
      Buffer.from(capture.data, 'base64')
    );
  }
}

socket.close();
console.log(JSON.stringify({ clips, fps, duration }, null, 2));
