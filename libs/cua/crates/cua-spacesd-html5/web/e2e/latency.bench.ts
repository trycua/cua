// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Click-to-pixel latency and CPU of a browser viewer, for comparing the
 * cua-spacesd HTML5 viewer with the old noVNC path (noVNC -> websockify ->
 * Xvnc). Runs in the Playwright container sharing the sandbox's network
 * namespace (see bench.sh). Headless Chromium, same machine, same guest
 * probe window for both.
 *
 * The guest runs a probe window (bench.sh starts it) that toggles between
 * black and white on every button press. The browser clicks it through the
 * viewer and times until the viewer's canvas shows the new color: input
 * path + guest redraw + capture + encode + transport + decode + paint.
 *
 * Env: MODE=viewer|novnc, CUA_ENV_TOKEN (viewer mode), N (samples), OUT.
 */

import { chromium } from "playwright";
import { writeFileSync } from "node:fs";

import { createApi } from "../src/api";

const MODE = process.env.MODE ?? "viewer";
const N = Number(process.env.N ?? 30);
const OUT = process.env.OUT ?? "/out";
const TOKEN = process.env.CUA_ENV_TOKEN ?? "";
// Probe window geometry in guest pixels (bench.sh places it at 0,0 800x600).
const PROBE = { x: 400, y: 300, dw: 1280, dh: 800 };

function pct(xs: number[], p: number): number {
  const s = [...xs].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.floor((p / 100) * s.length))] ?? NaN;
}

async function main() {
  let url: string;
  if (MODE === "viewer") {
    const root = createApi({ baseUrl: "http://127.0.0.1:3211/", ticket: TOKEN, params: new URLSearchParams() });
    const minted = await root.system.createViewerTicket({ ttl: { seconds: 900n, nanos: 0 }, clipboard: false, filesRoot: "" });
    url = `http://127.0.0.1:3211${minted.viewerPath}&audio=none`;
  } else {
    url = "http://127.0.0.1:6080/vnc.html?autoconnect=1&resize=scale&quality=6";
  }
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 860 } });
  await page.goto(url);
  const canvasSel = MODE === "viewer" ? "canvas.cua-screen" : "#noVNC_container canvas, canvas";
  await page.waitForSelector(canvasSel, { timeout: 30_000 });
  await page.waitForTimeout(4000);
  const box = (await page.locator(canvasSel).first().boundingBox())!;
  // Where the probe's centre lands on the canvas, in canvas pixels.
  const sample = await page.evaluate(
    ([sel, px, py, dw, dh]) => {
      const c = document.querySelector(sel as string) as HTMLCanvasElement;
      return { x: Math.floor(((px as number) / (dw as number)) * c.width), y: Math.floor(((py as number) / (dh as number)) * c.height) };
    },
    [canvasSel, PROBE.x + 60, PROBE.y + 60, PROBE.dw, PROBE.dh],
  );
  const read = () =>
    page.evaluate(
      ([sel, x, y]) => {
        const c = document.querySelector(sel as string) as HTMLCanvasElement;
        const ctx = c.getContext("2d", { willReadFrequently: true } as CanvasRenderingContext2DSettings)!;
        return ctx.getImageData(x as number, y as number, 1, 1).data[0]!;
      },
      [canvasSel, sample.x, sample.y],
    );
  const samples: number[] = [];
  const clickAt = { x: box.x + (PROBE.x / PROBE.dw) * box.width, y: box.y + (PROBE.y / PROBE.dh) * box.height };
  for (let i = 0; i < N + 3; i++) {
    const before = await read();
    // Time the click and the first changed pixel inside the page (one clock).
    const t0 = await page.evaluate(() => performance.now());
    await page.mouse.click(clickAt.x, clickAt.y);
    const ms = await page.evaluate(
      async ([sel, x, y, was, start]) => {
        const c = document.querySelector(sel as string) as HTMLCanvasElement;
        const ctx = c.getContext("2d", { willReadFrequently: true } as CanvasRenderingContext2DSettings)!;
        for (let k = 0; k < 400; k++) {
          const v = ctx.getImageData(x as number, y as number, 1, 1).data[0]!;
          if (Math.abs(v - (was as number)) > 100) return performance.now() - (start as number);
          await new Promise((r) => requestAnimationFrame(() => r(null)));
        }
        return -1;
      },
      [canvasSel, sample.x, sample.y, before, t0],
    );
    if (i >= 3 && ms > 0) samples.push(ms);
    await page.waitForTimeout(250);
  }
  const result = {
    mode: MODE,
    samples: samples.length,
    median_ms: pct(samples, 50),
    p90_ms: pct(samples, 90),
    min_ms: Math.min(...samples),
    max_ms: Math.max(...samples),
    all_ms: samples.map((x) => Math.round(x)),
  };
  console.log(JSON.stringify(result));
  writeFileSync(`${OUT}/latency-${MODE}.json`, JSON.stringify(result, null, 2));
  // Leave the page streaming for the CPU sample bench.sh takes next.
  if (process.env.HOLD_SECONDS) await page.waitForTimeout(Number(process.env.HOLD_SECONDS) * 1000);
  await browser.close();
}

main().catch((e) => {
  console.error(e);
  process.exit(2);
});
