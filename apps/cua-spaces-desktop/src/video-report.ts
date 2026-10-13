// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The video bench's report from two stats snapshots, for both apps: this
// shell's (`video-bench.ts`, from the page's counters) and the SwiftUI app's
// (`CUA_SPACES_VIDEO_STATS` in WebUIVideoBench.swift). The counts mean the
// same in both: `decoded` is frames out of the decoder, `presented` frames
// handed to the view, and fps is presented frames per second over the
// window, as apps/cua-spaces-macos/scripts/native-video-harness.sh computes
// it. Both time each presented frame from its arrival (`decodeMs`); only
// this shell has `received`, `painted` and arrival-to-paint (`latencyMs`),
// which the Swift app's report leaves null. No Electron here (scripts/video-bench.mjs
// runs it under Node).

/** One stream in a stats file (either app's). */
export interface StreamStatsLike {
  spaceId: string;
  tier: string;
  users?: number;
  decoded: number;
  presented: number;
  received?: number;
  painted?: number;
  size?: string;
  failure?: string | null;
  /** Arrival to paint per painted frame, newest last (ms). */
  latencyMs?: number[];
  /** Arrival to drawn per drawn frame, newest last (ms). */
  decodeMs?: number[];
}

/** A stats file: seconds since the bench started, and every stream. */
export interface StatsSnapshot {
  t: number;
  streams: StreamStatsLike[];
}

export interface Distribution {
  samples: number;
  mean: number;
  p50: number;
  p95: number;
  p99: number;
  max: number;
}

export interface StreamReport {
  spaceId: string;
  tier: string;
  /** Presented frames per second over the window. */
  fps: number;
  decoded: number;
  presented: number;
  received: number | null;
  painted: number | null;
  /** Frames that arrived but never reached a paint (null: not counted). */
  dropped: number | null;
  droppedPercent: number | null;
  latencyMs: Distribution | null;
  decodeMs: Distribution | null;
  size: string;
  failure: string | null;
}

export interface VideoReport {
  app: string;
  seconds: number;
  /** Mean CPU over the window, % of one core (the app and its helper processes). */
  cpuPercent: number | null;
  cpuMax: number | null;
  memoryMB: number | null;
  tiles: StreamReport[];
  full: StreamReport[];
}

const round = (v: number, places = 1) => {
  const k = 10 ** places;
  return Math.round(v * k) / k;
};

/** Nearest-rank percentile of sorted values. */
export function percentile(sorted: readonly number[], p: number): number {
  if (sorted.length === 0) return 0;
  const rank = Math.ceil((p / 100) * sorted.length);
  return sorted[Math.min(sorted.length - 1, Math.max(0, rank - 1))]!;
}

export function distribution(values: readonly number[]): Distribution | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const mean = sorted.reduce((a, b) => a + b, 0) / sorted.length;
  return {
    samples: sorted.length,
    mean: round(mean),
    p50: round(percentile(sorted, 50)),
    p95: round(percentile(sorted, 95)),
    p99: round(percentile(sorted, 99)),
    max: round(sorted.at(-1)!),
  };
}

/** The newest `count` samples of `after` (those added since the window began). */
function windowSamples(list: number[] | undefined, count: number): number[] {
  if (!list || count <= 0) return [];
  return list.slice(-Math.min(count, list.length));
}

/** A Space's streams at one tier, merged (several slots or windows on one stream). */
function merge(streams: readonly StreamStatsLike[]): Map<string, StreamStatsLike> {
  const out = new Map<string, StreamStatsLike>();
  for (const s of streams) {
    const key = `${s.spaceId}\u0000${s.tier}`;
    const held = out.get(key);
    if (!held) {
      out.set(key, { ...s, latencyMs: [...(s.latencyMs ?? [])], decodeMs: [...(s.decodeMs ?? [])] });
      continue;
    }
    held.decoded += s.decoded;
    held.presented += s.presented;
    if (s.received !== undefined) held.received = (held.received ?? 0) + s.received;
    if (s.painted !== undefined) held.painted = (held.painted ?? 0) + s.painted;
    held.latencyMs!.push(...(s.latencyMs ?? []));
    held.decodeMs!.push(...(s.decodeMs ?? []));
    held.failure ??= s.failure ?? null;
    if (!held.size) held.size = s.size;
  }
  return out;
}

/** One stream over the window from `before` to `after` (`before` absent: it started inside the window). */
export function streamReport(before: StreamStatsLike | undefined, after: StreamStatsLike, seconds: number): StreamReport {
  const presented = after.presented - (before?.presented ?? 0);
  const decoded = after.decoded - (before?.decoded ?? 0);
  const received = after.received === undefined ? null : after.received - (before?.received ?? 0);
  const painted = after.painted === undefined ? null : after.painted - (before?.painted ?? 0);
  const dropped = received === null || painted === null ? null : Math.max(0, received - painted);
  return {
    spaceId: after.spaceId,
    tier: after.tier,
    fps: seconds > 0 ? round(presented / seconds) : 0,
    decoded,
    presented,
    received,
    painted,
    dropped,
    droppedPercent: dropped === null || !received ? null : round((100 * dropped) / received),
    latencyMs: painted === null ? null : distribution(windowSamples(after.latencyMs, painted)),
    decodeMs: distribution(windowSamples(after.decodeMs, presented)),
    size: after.size ?? "",
    failure: after.failure ?? null,
  };
}

/** The report over the window between two snapshots, with the CPU (% of one core) and memory (MB) sampled meanwhile. */
export function videoReport(app: string, before: StatsSnapshot, after: StatsSnapshot, load: { cpu: readonly number[]; memoryMB: readonly number[] }): VideoReport {
  const seconds = Math.max(0, after.t - before.t);
  const was = merge(before.streams);
  const streams = [...merge(after.streams).entries()].map(([key, s]) => streamReport(was.get(key), s, seconds));
  const mean = (a: readonly number[]) => (a.length ? round(a.reduce((x, y) => x + y, 0) / a.length) : null);
  return {
    app,
    seconds: round(seconds),
    cpuPercent: mean(load.cpu),
    cpuMax: load.cpu.length ? round(Math.max(...load.cpu)) : null,
    memoryMB: mean(load.memoryMB) === null ? null : Math.round(mean(load.memoryMB)!),
    tiles: streams.filter((s) => s.tier === "tile"),
    full: streams.filter((s) => s.tier !== "tile"),
  };
}
