// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The video bench's counters for the page's own decoding (WebCodecs, in the
 * Electron shell): per stream, frames received, frames drawn onto the
 * canvas, frames the compositor painted, and how long each painted frame
 * took from its packet's arrival to the paint. Collected only while the
 * shell asks for it (`cuaDesktop.videoStats`, under the bench's
 * `CUA_SPACES_VIDEO_STATS`); the shell reads `snapshot()` once a second
 * (apps/cua-spaces-desktop/src/video-bench.ts).
 *
 * The counts match the SwiftUI app's `WebUIVideoBench` stats: `decoded` is
 * frames out of the decoder and `presented` frames handed to the view (the
 * canvas here, the layer there), so fps is presented frames per second in
 * both apps. `painted` and the latency are this page's own: a frame drawn
 * over before the next paint never reached the screen (`dropped`).
 */

import type { FrameTiming } from "../../../../../libs/cua/crates/cua-spacesd-html5/web/src/core/mediaSession";

/** The most latency samples kept per stream (two minutes at 60 fps). */
export const LATENCY_SAMPLES = 7200;

export interface StreamStats {
  spaceId: string;
  tier: string;
  /** Slots showing it (1 here: every slot has its own session). */
  users: number;
  /** Frames whose packet arrived. */
  received: number;
  /** Frames out of the decoder. */
  decoded: number;
  /** Frames handed to the view (drawn onto the canvas). */
  presented: number;
  /** Frames on screen at a paint. */
  painted: number;
  /** Frames drawn over before a paint, or never decoded (received - painted). */
  dropped: number;
  /** The stream's size, `<w>x<h>`, once known. */
  size: string;
  failure: string | null;
  /** Arrival to paint, per painted frame (ms; the newest `LATENCY_SAMPLES`). */
  latencyMs: number[];
  /** Arrival to drawn, per drawn frame (ms; the newest `LATENCY_SAMPLES`). */
  decodeMs: number[];
}

interface Entry extends StreamStats {
  /** The newest frame drawn since the last paint. */
  pending: FrameTiming | null;
}

export interface FrameClock {
  now(): number;
  /** The next paint (`requestAnimationFrame`): its time. */
  nextPaint(callback: (time: number) => void): number;
  cancel(handle: number): void;
}

const browserClock = (): FrameClock => ({
  now: () => performance.now(),
  nextPaint: (cb) => requestAnimationFrame(cb),
  cancel: (h) => cancelAnimationFrame(h),
});

const push = (list: number[], v: number) => {
  list.push(v);
  if (list.length > LATENCY_SAMPLES) list.splice(0, list.length - LATENCY_SAMPLES);
};

export class VideoStats {
  private readonly entries = new Map<string, Entry>();
  private frame: number | null = null;

  constructor(private readonly clock: FrameClock = browserClock()) {}

  /** A stream's counters, under the slot's id. */
  open(id: string, spaceId: string, tier: string): void {
    this.entries.set(id, {
      spaceId,
      tier,
      users: 1,
      received: 0,
      decoded: 0,
      presented: 0,
      painted: 0,
      dropped: 0,
      size: "",
      failure: null,
      latencyMs: [],
      decodeMs: [],
      pending: null,
    });
  }

  close(id: string): void {
    this.entries.delete(id);
    if (this.entries.size === 0 && this.frame !== null) {
      this.clock.cancel(this.frame);
      this.frame = null;
    }
  }

  /** The session's own counts (received and decoded frames, the size). */
  counts(id: string, c: { received: number; decoded: number; width?: number; height?: number }): void {
    const e = this.entries.get(id);
    if (!e) return;
    e.received = c.received;
    e.decoded = c.decoded;
    if (c.width && c.height) e.size = `${c.width}x${c.height}`;
    e.dropped = Math.max(0, e.received - e.painted);
  }

  failed(id: string, reason: string): void {
    const e = this.entries.get(id);
    if (e) e.failure = reason;
  }

  /** A frame was drawn onto the slot's canvas. */
  drawn(id: string, t: FrameTiming): void {
    const e = this.entries.get(id);
    if (!e) return;
    e.presented += 1;
    push(e.decodeMs, t.drawnAt - t.receivedAt);
    e.pending = t;
    this.frame ??= this.clock.nextPaint(this.paint);
  }

  private paint = (time: number): void => {
    this.frame = null;
    for (const e of this.entries.values()) {
      if (!e.pending) continue;
      e.painted += 1;
      push(e.latencyMs, Math.max(0, time - e.pending.receivedAt));
      e.pending = null;
      e.dropped = Math.max(0, e.received - e.painted);
    }
  };

  /** Every stream's counters now (copies). */
  snapshot(): StreamStats[] {
    return [...this.entries.values()].map(({ pending: _, latencyMs, decodeMs, ...e }) => ({ ...e, latencyMs: [...latencyMs], decodeMs: [...decodeMs] }));
  }
}
