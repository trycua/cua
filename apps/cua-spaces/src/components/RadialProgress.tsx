// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useRef, useState } from "react";

/**
 * A macOS-installer-style radial progress pie (a filling disc, inner radius 0).
 * There is no real percentage to report while a Space starts, so it eases toward
 * completion on an estimate: ~80% by ~80s (the measured bind time), then slows
 * and asymptotes so it never reaches 100% until the work actually finishes.
 *
 * `startedAt` (epoch ms) anchors elapsed time to the real creation moment
 * so the pie doesn't reset when the UI is closed and reopened; without it the
 * clock starts at mount.
 */
export function RadialProgress({
  startedAt,
  className,
  fraction,
  estimateMs = 80_000,
}: {
  startedAt?: number;
  className?: string;
  /** A real 0–1 progress (e.g. bytes sent/total). When set, the pie shows it
   * directly instead of the time-based estimate. */
  fraction?: number;
  /** Rough time to ~80% for the time-based estimate. Default ~80s (Fleet bind
   * time); a Local Lume Space is much faster (~17s to running). */
  estimateMs?: number;
}) {
  const [progress, setProgress] = useState(0.04);
  const mountedAt = useRef<number | null>(null);

  useEffect(() => {
    if (fraction !== undefined) return; // determinate: driven by the prop below
    // TAU chosen so the ease hits ~80% at `estimateMs` (1 - e^-1.609 ≈ 0.8).
    const TAU = estimateMs / 1.609;
    const CAP = 0.96;
    const tick = () => {
      const now = Date.now();
      if (mountedAt.current === null) mountedAt.current = now;
      const elapsed = now - (startedAt ?? mountedAt.current);
      setProgress(Math.max(0.04, Math.min(CAP, 1 - Math.exp(-Math.max(0, elapsed) / TAU))));
    };
    tick();
    const timer = window.setInterval(tick, 200);
    return () => window.clearInterval(timer);
  }, [startedAt, fraction, estimateMs]);

  const value = fraction !== undefined ? Math.max(0.04, Math.min(1, fraction)) : progress;

  return (
    <span
      className={className ? `radial-pie ${className}` : "radial-pie"}
      style={{ ["--progress" as string]: String(value) }}
      aria-hidden="true"
    />
  );
}
