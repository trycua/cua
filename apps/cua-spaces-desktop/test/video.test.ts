// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Live video in the shell, without Electron: Chromium's hardware decode
// switches per platform (gpu.ts), the bench's environment (video-bench.ts)
// and its report math (video-report.ts), with the SwiftUI harness's
// definitions (fps is presented frames per second over the window).
import { describe, expect, it } from "vitest";
import { LINUX_DECODE_FEATURES, applyVideoDecodeSwitches, videoDecodeSwitches } from "../src/gpu";
import { countsVideo, videoBenchEnv } from "../src/video-bench";
import { distribution, percentile, streamReport, videoReport, type StatsSnapshot } from "../src/video-report";

describe("hardware video decode", () => {
  it("leaves macOS and Windows to Chromium's defaults (VideoToolbox, Media Foundation)", () => {
    expect(videoDecodeSwitches("darwin", {}, "")).toEqual([]);
    expect(videoDecodeSwitches("win32", {}, "")).toEqual([]);
  });

  it("turns VA-API decode on for Linux, keeping features already asked for", () => {
    expect(videoDecodeSwitches("linux", {}, "")).toEqual([["enable-features", LINUX_DECODE_FEATURES.join(",")]]);
    expect(videoDecodeSwitches("linux", {}, "WaylandWindowDecorations,AcceleratedVideoDecodeLinuxGL")).toEqual([
      ["enable-features", "WaylandWindowDecorations,AcceleratedVideoDecodeLinuxGL,AcceleratedVideoDecodeLinuxZeroCopyGL"],
    ]);
  });

  it("decodes in software on every platform when asked", () => {
    for (const p of ["darwin", "win32", "linux"] as const) expect(videoDecodeSwitches(p, { CUA_SPACES_VIDEO_DECODE: "software" }, "")).toEqual([["disable-accelerated-video-decode"]]);
  });

  it("appends them to Electron's command line", () => {
    const added: [string, string?][] = [];
    applyVideoDecodeSwitches({ getSwitchValue: () => "", appendSwitch: (n, v) => void added.push([n, v]) }, "linux", {});
    expect(added).toEqual([["enable-features", LINUX_DECODE_FEATURES.join(",")]]);
  });
});

describe("the video bench's environment", () => {
  it("reads the Swift app's hook names, with the report's window", () => {
    expect(videoBenchEnv({})).toEqual({ bench: null, stats: null, report: null, warmupSeconds: 12, seconds: 30 });
    const b = videoBenchEnv({ CUA_SPACES_VIDEO_BENCH: "local:a", CUA_SPACES_VIDEO_REPORT: "/r.json", CUA_SPACES_VIDEO_SECONDS: "20", CUA_SPACES_VIDEO_WARMUP: "x" });
    expect(b).toEqual({ bench: "local:a", stats: null, report: "/r.json", warmupSeconds: 12, seconds: 20 });
    expect(countsVideo(b)).toBe(true);
    expect(countsVideo(videoBenchEnv({ CUA_SPACES_VIDEO_BENCH: "local:a" }))).toBe(false);
  });
});

describe("the video bench's report", () => {
  it("takes nearest-rank percentiles", () => {
    const sorted = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
    expect([percentile(sorted, 50), percentile(sorted, 95), percentile(sorted, 99), percentile(sorted, 100)]).toEqual([5, 10, 10, 10]);
    expect(distribution([30, 10, 20])).toEqual({ samples: 3, mean: 20, p50: 20, p95: 30, p99: 30, max: 30 });
    expect(distribution([])).toBeNull();
  });

  it("counts fps as frames presented per second over the window, and drops as arrived but never painted", () => {
    const before = { spaceId: "local:a", tier: "full", decoded: 100, presented: 100, received: 102, painted: 98, latencyMs: [50, 50], decodeMs: [5] };
    const after = { spaceId: "local:a", tier: "full", decoded: 700, presented: 700, received: 712, painted: 688, latencyMs: [50, 50, 20, 22, 24, 30], decodeMs: [5, 4, 6], size: "1920x1080", failure: null };
    const r = streamReport(before, after, 10);
    expect(r).toMatchObject({ fps: 60, decoded: 600, presented: 600, received: 610, painted: 590, dropped: 20, droppedPercent: 3.3, size: "1920x1080" });
    // Only the window's samples (the newest `painted`), as many as are kept.
    expect(r.latencyMs).toMatchObject({ samples: 6, max: 50 });
    expect(streamReport(undefined, { ...after, latencyMs: [10, 20, 30] }, 10).latencyMs).toMatchObject({ samples: 3, p50: 20 });
  });

  it("reads the SwiftUI app's stats, which have no arrivals, paints or latencies", () => {
    const before: StatsSnapshot = { t: 12, streams: [{ spaceId: "local:a", tier: "tile", users: 1, decoded: 120, presented: 120, size: "960x600" }] };
    const after: StatsSnapshot = {
      t: 42,
      streams: [
        { spaceId: "local:a", tier: "tile", users: 1, decoded: 420, presented: 420, size: "960x600" },
        { spaceId: "local:b", tier: "full", users: 1, decoded: 1795, presented: 1790, size: "1920x1080", failure: null },
      ],
    };
    const r = videoReport("swift", before, after, { cpu: [12, 14, 16], memoryMB: [270, 272] });
    expect(r).toMatchObject({ app: "swift", seconds: 30, cpuPercent: 14, cpuMax: 16, memoryMB: 271 });
    expect(r.tiles).toEqual([
      { spaceId: "local:a", tier: "tile", fps: 10, decoded: 300, presented: 300, received: null, painted: null, dropped: null, droppedPercent: null, latencyMs: null, decodeMs: null, size: "960x600", failure: null },
    ]);
    expect(r.full[0]).toMatchObject({ spaceId: "local:b", fps: 59.7, presented: 1790 });
  });

  it("times the SwiftUI app's presented frames from their arrival, as this app's drawn ones", () => {
    const s = { spaceId: "local:b", tier: "full", decoded: 4, presented: 4, decodeMs: [9, 4, 6, 5, 7] };
    const r = videoReport("swift", { t: 0, streams: [] }, { t: 1, streams: [s] }, { cpu: [], memoryMB: [] });
    expect(r.full[0]!.decodeMs).toEqual({ samples: 4, mean: 5.5, p50: 5, p95: 7, p99: 7, max: 7 });
    expect(r.full[0]!.latencyMs).toBeNull();
  });

  it("merges a Space's streams at one tier, and says when nothing was sampled", () => {
    const s = { spaceId: "local:a", tier: "tile", decoded: 10, presented: 10, received: 10, painted: 10 };
    const r = videoReport("electron", { t: 0, streams: [] }, { t: 1, streams: [s, s] }, { cpu: [], memoryMB: [] });
    expect(r.tiles).toHaveLength(1);
    expect(r.tiles[0]).toMatchObject({ fps: 20, received: 20, painted: 20, dropped: 0 });
    expect([r.cpuPercent, r.cpuMax, r.memoryMB]).toEqual([null, null, null]);
  });
});
