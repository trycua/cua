// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The video bench's hooks in this shell, the same as the SwiftUI app's
// WebUIVideoBench.swift so both apps run side by side on one Space
// (docs/video.md):
//
// - `CUA_SPACES_VIDEO_BENCH=<spaceId>`: beside the main window (the Spaces
//   grid, its tiles), that Space's viewer window ("Open in window"), which
//   streams it at full rate and size (the Swift app's second window is a
//   web UI window on its page, at the same tier); both placed as the Swift
//   app places them, and neither placement saved over the person's
//   (main.ts).
// - `CUA_SPACES_VIDEO_STATS=<file>`: once a second, every stream's counts
//   from the pages (`video-stats.ts` in the web UI), as JSON.
// - `CUA_SPACES_VIDEO_REPORT=<file>`: after `CUA_SPACES_VIDEO_WARMUP`
//   seconds (12), measures for `CUA_SPACES_VIDEO_SECONDS` (30), writes the
//   report (`video-report.ts`, with the app's CPU and memory from
//   `app.getAppMetrics`) and quits.
// Both say whether Chromium decodes video on the GPU here (`videoDecode`).
import { app, BrowserWindow } from "electron";
import { writeFileSync } from "node:fs";
import type { StatsSnapshot, StreamStatsLike } from "./video-report";
import { videoReport } from "./video-report";

export interface VideoBenchEnv {
  bench: string | null;
  stats: string | null;
  report: string | null;
  warmupSeconds: number;
  seconds: number;
}

export function videoBenchEnv(env: NodeJS.ProcessEnv): VideoBenchEnv {
  const text = (k: string) => (env[k] && env[k] !== "" ? env[k]! : null);
  const seconds = (k: string, fallback: number) => {
    const n = Number(env[k]);
    return Number.isFinite(n) && n > 0 ? n : fallback;
  };
  return {
    bench: text("CUA_SPACES_VIDEO_BENCH"),
    stats: text("CUA_SPACES_VIDEO_STATS"),
    report: text("CUA_SPACES_VIDEO_REPORT"),
    warmupSeconds: seconds("CUA_SPACES_VIDEO_WARMUP", 12),
    seconds: seconds("CUA_SPACES_VIDEO_SECONDS", 30),
  };
}

/** Whether the pages count their video (`cuaDesktop.videoStats`). */
export const countsVideo = (b: VideoBenchEnv) => b.stats !== null || b.report !== null;

/** Every page's streams now. */
async function streams(): Promise<StreamStatsLike[]> {
  const all = await Promise.all(
    BrowserWindow.getAllWindows().map((w) =>
      w.isDestroyed()
        ? []
        : (w.webContents.executeJavaScript("window.__cuaVideoStats ? window.__cuaVideoStats() : []", false).catch(() => []) as Promise<StreamStatsLike[]>),
    ),
  );
  return all.flat();
}

/** The app's CPU (% of one core, every process) and memory (MB) now. */
function load(): { cpu: number; memoryMB: number } {
  const metrics = app.getAppMetrics();
  return {
    cpu: metrics.reduce((n, m) => n + m.cpu.percentCPUUsage, 0),
    memoryMB: metrics.reduce((n, m) => n + m.memory.workingSetSize, 0) / 1024,
  };
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** Chromium's video decode status (`chrome://gpu`): `enabled` is on the GPU, `unavailable_software` is not. */
const videoDecode = () => app.getGPUFeatureStatus().video_decode;

/** Starts the hooks the environment asks for, once the main window is up. */
export function startVideoBench(b: VideoBenchEnv, o: { main: BrowserWindow; openSpace: (spaceId: string) => BrowserWindow }): void {
  if (b.bench) {
    const viewer = o.openSpace(b.bench);
    o.main.setBounds({ x: 40, y: 80, width: 1240, height: 860 });
    viewer.setBounds({ x: 1300, y: 80, width: 900, height: 700 });
  }
  const start = Date.now();
  const t = () => (Date.now() - start) / 1000;
  if (b.stats) {
    const file = b.stats;
    void (async () => {
      for (;;) {
        await sleep(1000);
        const out: StatsSnapshot & { load: ReturnType<typeof load>; videoDecode: string } = { t: t(), streams: await streams(), load: load(), videoDecode: videoDecode() };
        try {
          writeFileSync(file, JSON.stringify(out));
        } catch (error) {
          console.warn(`[cua-spaces] video bench: could not write ${file}:`, error);
        }
      }
    })();
  }
  if (b.report) {
    const file = b.report;
    void (async () => {
      await sleep(b.warmupSeconds * 1000);
      load(); // the first reading's CPU counts since launch
      const before: StatsSnapshot = { t: t(), streams: await streams() };
      const cpu: number[] = [];
      const memoryMB: number[] = [];
      for (let i = 0; i < b.seconds; i++) {
        await sleep(1000);
        const l = load();
        cpu.push(l.cpu);
        memoryMB.push(l.memoryMB);
      }
      const after: StatsSnapshot = { t: t(), streams: await streams() };
      const report = { ...videoReport("electron", before, after, { cpu, memoryMB }), videoDecode: videoDecode(), platform: process.platform, arch: process.arch, bench: b.bench };
      writeFileSync(file, `${JSON.stringify(report, null, 2)}\n`);
      app.quit();
    })();
  }
}
