// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Records the demo walkthrough of the web UI in Chromium (demo bridge) to
// docs/demo/spaces-new-ui-demo.mp4 at 1440x900.
// Usage: pnpm build && node scripts/demo-video.mjs   (needs ffmpeg on PATH)
//
// Frames come from the DevTools screencast with their timestamps, so pauses
// keep their real length. The pointer drawn in the page is a scripted one for
// the recording; headless Chromium has no system cursor to show. One page
// load, no reloads: the live agent turn replays by rewinding the clock the
// demo adapter reads (see __demoRewind).
import { spawn, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { chromium } from "@playwright/test";

const root = fileURLToPath(new URL("..", import.meta.url));
const out = `${root}docs/demo/spaces-new-ui-demo.mp4`;
const W = 1440;
const H = 900;

const port = 5177;
const base = `http://localhost:${port}`;
const server = spawn("npx", ["vite", "preview", "--port", String(port), "--strictPort"], { cwd: root, stdio: "ignore" });
for (let i = 0; i < 50; i++) {
  try {
    if ((await fetch(base)).ok) break;
  } catch {}
  await new Promise((r) => setTimeout(r, 200));
}

const frameDir = await mkdtemp(join(tmpdir(), "cua-demo-"));
const frames = [];
const browser = await chromium.launch();
try {
  const context = await browser.newContext({ viewport: { width: W, height: H }, deviceScaleFactor: 1, colorScheme: "light" });
  await context.addInitScript(() => {
    localStorage.setItem("cua-spaces:theme", "light");
    // The demo adapter reads Date.now; __demoRewind moves it back to the
    // adapter's start so the live agent turn plays again.
    const real = Date.now.bind(Date);
    const loaded = real();
    let offset = 0;
    Date.now = () => real() - offset;
    window.__demoRewind = () => (offset = real() - loaded);
    const draw = () => {
      if (document.getElementById("demo-pointer")) return;
      const p = document.createElement("div");
      p.id = "demo-pointer";
      p.innerHTML =
        '<svg width="22" height="22" viewBox="0 0 22 22"><path d="M3 2 L3 18 L7.5 13.8 L10.6 20.4 L13.2 19.2 L10.2 12.8 L16.4 12.6 Z" fill="#111" stroke="#fff" stroke-width="1.4" stroke-linejoin="round"/></svg>';
      Object.assign(p.style, { position: "fixed", left: "0", top: "0", zIndex: "2147483647", pointerEvents: "none", transform: "translate(-100px,-100px)" });
      document.documentElement.appendChild(p);
      const ring = document.createElement("div");
      Object.assign(ring.style, {
        position: "fixed", width: "28px", height: "28px", margin: "-14px 0 0 -14px", borderRadius: "50%",
        border: "2px solid rgba(80,120,255,.7)", zIndex: "2147483646", pointerEvents: "none", opacity: "0",
        transition: "opacity .35s, transform .35s",
      });
      document.documentElement.appendChild(ring);
      addEventListener("mousemove", (e) => (p.style.transform = `translate(${e.clientX - 3}px,${e.clientY - 2}px)`), true);
      addEventListener("mousedown", (e) => {
        Object.assign(ring.style, { left: `${e.clientX}px`, top: `${e.clientY}px`, transition: "none", opacity: "1", transform: "scale(.6)" });
        requestAnimationFrame(() => Object.assign(ring.style, { transition: "opacity .45s, transform .45s", opacity: "0", transform: "scale(1.4)" }));
      }, true);
    };
    if (document.readyState === "loading") addEventListener("DOMContentLoaded", draw);
    else draw();
  });
  const page = await context.newPage();
  const cdp = await context.newCDPSession(page);
  let n = 0;
  cdp.on("Page.screencastFrame", async ({ data, metadata, sessionId }) => {
    const file = join(frameDir, `f${String(n++).padStart(5, "0")}.jpg`);
    frames.push({ file, t: metadata.timestamp });
    await writeFile(file, Buffer.from(data, "base64"));
    await cdp.send("Page.screencastFrameAck", { sessionId }).catch(() => {});
  });

  let mouse = { x: W / 2, y: H / 2 };
  // Holds scale together so the walkthrough stays around 90 s.
  const PACE = 0.68;
  const pause = (ms) => page.waitForTimeout(ms * PACE);
  const glide = async (x, y, ms = 550) => {
    const from = { ...mouse };
    const steps = Math.max(8, Math.round(ms / 16));
    for (let i = 1; i <= steps; i++) {
      const k = i / steps;
      const e = k < 0.5 ? 2 * k * k : 1 - (-2 * k + 2) ** 2 / 2;
      await page.mouse.move(from.x + (x - from.x) * e, from.y + (y - from.y) * e);
      await page.waitForTimeout(16);
    }
    mouse = { x, y };
  };
  const point = async (locator) => {
    const box = await locator.boundingBox();
    if (!box) throw new Error(`not on screen: ${locator}`);
    await glide(box.x + box.width / 2, box.y + box.height / 2);
  };
  const click = async (locator) => {
    await point(locator);
    await page.waitForTimeout(180);
    await page.mouse.down();
    await page.mouse.up();
  };
  const type = (text) => page.keyboard.type(text, { delay: 95 });
  // The content pane keeps its scroll across routes; start each page at the top.
  const nav = async (name) => {
    await click(page.locator("aside, nav").getByRole("link", { name, exact: false }).first());
    await page.evaluate(() => document.querySelectorAll("*").forEach((el) => el.scrollTop && (el.scrollTop = 0)));
  };

  const settingsTab = (name) => click(page.locator('nav[aria-label="Settings"]').getByRole("link", { name, exact: true }));
  const next = () => click(page.locator("[data-wizard-primary]"));
  const scrollTo = async (locator) => {
    await locator.evaluate((el) => el.scrollIntoView({ behavior: "smooth", block: "center" }));
    await pause(900);
  };
  const volumeSwitch = page.locator('[data-setting-row="experiment:cua_volume"] [data-slot="switch"]');

  // Off camera: the demo starts with Cua Volume on, so turn it off to show
  // turning it on later. Client-side navigation keeps that state.
  await page.goto(`${base}/settings/experiments`, { waitUntil: "networkidle" });
  if ((await volumeSwitch.getAttribute("aria-checked")) === "true") await volumeSwitch.click();
  // Share shows only with Settings, Experiments, Sharing on (as in the apps).
  const sharingSwitch = page.locator('[data-setting-row="experiment:sharing"] [data-slot="switch"]');
  if ((await sharingSwitch.getAttribute("aria-checked")) !== "true") await sharingSwitch.click();
  await page.locator("aside, nav").getByRole("link", { name: "Spaces", exact: false }).first().click();
  await page.locator("[data-space-id]").first().waitFor();
  await page.mouse.move(mouse.x, mouse.y);
  await pause(600);
  await cdp.send("Page.startScreencast", { format: "jpeg", quality: 92, maxWidth: W, maxHeight: H, everyNthFrame: 1 });

  try {
  // Spaces grid, then New Space: System, Run on, Resources, name, create.
  await pause(2500);
  await click(page.getByRole("button", { name: "New Space" }).first());
  await pause(2200);
  await click(page.locator("[data-run-on]"));
  await pause(2200);
  await page.keyboard.press("Escape");
  await pause(600);
  await click(page.locator("[data-tile='os:macos']"));
  await pause(1200);
  await next();
  await pause(1500);
  await click(page.locator("[data-gpu]"));
  await pause(1800);
  await next();
  await pause(900);
  await click(page.locator("[data-name-input]"));
  await page.locator("[data-name-input]").fill("");
  await type("release-mac");
  await pause(900);
  await next();
  await pause(2400);
  await next();
  await pause(4500);

  // A Space's detail: facts, Stream, Share, then Teleport an app to review.
  await click(page.locator('[data-space-id="local:design-review"]'));
  await pause(2800);
  const stream = page.getByText("Stream", { exact: true }).first();
  if (await stream.count()) {
    await scrollTo(stream);
    await pause(2200);
    await page.evaluate(() => document.querySelector("main")?.scrollTo({ top: 0, behavior: "smooth" }) ?? scrollTo({ top: 0, behavior: "smooth" }));
    await pause(1000);
  }
  await point(page.getByRole("button", { name: "Share", exact: true }).first());
  await pause(1200);
  await click(page.getByRole("button", { name: "Teleport an app" }));
  await page.locator("[data-teleport][data-step=pick]").waitFor();
  await pause(2400);
  await point(page.locator("[data-tile='com.google.Chrome']"));
  await pause(300);
  await page.locator("[data-tile='com.google.Chrome']").dblclick();
  await pause(1400);
  await click(page.getByText("The app with its signed-in state"));
  await pause(1000);
  await click(page.getByText("Keep me signed in"));
  await pause(1200);
  await click(page.locator("[data-teleport-plan]"));
  await page.locator("[data-review-site]").first().waitFor();
  await pause(3200);
  await page.keyboard.press("Escape");
  await pause(900);

  // The demo's live agent turn is written over the first ~25 s after the
  // adapter starts. Rewind the clock the adapter reads so it streams when
  // Agents opens, without reloading the page.
  await page.evaluate(() => window.__demoRewind());

  // Machines: This machine.
  await nav("Machines");
  await pause(2200);
  await scrollTo(page.locator("[data-host-action]").first());
  await pause(2600);

  // Agents, streaming.
  await nav("Agents");
  await pause(900);
  await click(page.getByText("ada", { exact: true }).first());
  await page.waitForTimeout(9000);

  // Keyvault: search.
  await nav("Keyvault");
  await pause(2200);
  await click(page.getByRole("textbox", { name: "Search Keyvault" }));
  await type("github");
  await pause(2400);

  // Settings, Experiments: turn on Cua Volume, then open Volume.
  await nav("Settings");
  await pause(1400);
  await settingsTab("Experiments");
  await pause(1600);
  await click(volumeSwitch);
  await pause(1600);
  await nav("Volume");
  await pause(3600);

  // Settings: About and Devices.
  await nav("Settings");
  await pause(900);
  await settingsTab("About");
  await pause(2600);
  await settingsTab("Devices");
  await pause(2800);

  // Notifications.
  await nav("Notifications");
  await pause(3200);

  // A glimpse of the first run.
  await page.keyboard.press("Meta+k");
  await pause(700);
  await type("Onboarding");
  await pause(900);
  await page.keyboard.press("Enter");
  await pause(2600);
  await click(page.getByRole("button", { name: "Get started" }));
  await pause(3200);

  } catch (e) {
    await page.screenshot({ path: join(tmpdir(), "cua-demo-fail.png") });
    throw e;
  }
  await cdp.send("Page.stopScreencast");
  await context.close();
} finally {
  await browser.close();
  server.kill();
}

// Each frame lasts until the next one arrived.
const list = frames
  .map((f, i) => `file '${f.file}'\nduration ${((frames[i + 1]?.t ?? f.t + 1) - f.t).toFixed(4)}`)
  .join("\n");
await writeFile(join(frameDir, "list.txt"), `${list}\nfile '${frames.at(-1).file}'\n`);
await mkdir(`${root}docs/demo`, { recursive: true });
const ff = spawnSync(
  "ffmpeg",
  ["-y", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i", join(frameDir, "list.txt"),
    "-vf", `fps=30,scale=${W}:${H}:flags=lanczos:out_range=tv,format=yuv420p`, "-c:v", "libx264", "-preset", "slow", "-crf", "24",
    "-pix_fmt", "yuv420p", "-movflags", "+faststart", out],
  { stdio: "inherit" },
);
await rm(frameDir, { recursive: true, force: true });
if (ff.status !== 0) process.exit(ff.status ?? 1);
console.log(`Saved ${out} (${frames.length} frames)`);
