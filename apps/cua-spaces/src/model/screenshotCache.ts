// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Shared, app-wide "last screenshot" cache: `spaceId -> { dataUrl, ts }`.
 *
 * ONE cache feeds every surface that wants a recent frame of a Space without
 * waiting for a live stream:
 *   - written by the tile thumbnail poll (`useLiveThumbnail`) and the new
 *     hover preload (item C),
 *   - read by the Space viewer's connecting background (item D) and the
 *     transfer overlay (item B).
 *
 * The viewer runs in a separate webview, so writes are also mirrored into the
 * Rust shell (`cache_space_screenshot`), which hands the latest frame back to
 * viewer windows through `viewer_config`. The in-memory map here keeps the
 * writer window (the portal) fast and makes the cache unit-testable with no
 * Tauri and no network. Pure map + subscribe; the mirror is best-effort.
 */

import { hasTauri } from "../native/bridge";

export interface ScreenshotEntry {
  /** `data:image/png;base64,…` URL of the most recent frame. */
  dataUrl: string;
  /** When it was captured (ms epoch, from the caller's clock). */
  ts: number;
}

/** A frame older than this is treated as stale for the connecting background. */
export const DEFAULT_SCREENSHOT_MAX_AGE_MS = 60_000;

const cache = new Map<string, ScreenshotEntry>();
const subscribers = new Set<() => void>();

function notify(): void {
  for (const fn of [...subscribers]) fn();
}

/** Best-effort mirror to the shell so viewer windows can read the frame too. */
function mirrorToShell(spaceId: string, dataUrl: string): void {
  if (!hasTauri()) return;
  void import("@tauri-apps/api/core")
    .then(({ invoke }) => invoke("cache_space_screenshot", { spaceId, dataUrl }))
    .catch(() => {
      // The shell may not be ready or the command may be unavailable; the
      // in-memory cache still serves same-window readers.
    });
}

/**
 * Record the newest frame for a Space. Older timestamps are ignored so an
 * out-of-order poll cannot clobber a fresher hover preload (and vice-versa).
 */
export function rememberScreenshot(spaceId: string, dataUrl: string, ts: number): void {
  if (!spaceId || !dataUrl) return;
  const existing = cache.get(spaceId);
  if (existing && existing.ts > ts) return;
  cache.set(spaceId, { dataUrl, ts });
  mirrorToShell(spaceId, dataUrl);
  notify();
}

/** The latest cached frame for a Space, or null when nothing is cached. */
export function getScreenshot(spaceId: string): ScreenshotEntry | null {
  return cache.get(spaceId) ?? null;
}

/**
 * The latest frame only when it is younger than `maxAgeMs`; null otherwise.
 * Used by the connecting background so a very stale frame is not shown.
 */
export function getRecentScreenshot(
  spaceId: string,
  now: number,
  maxAgeMs: number = DEFAULT_SCREENSHOT_MAX_AGE_MS,
): ScreenshotEntry | null {
  const entry = cache.get(spaceId);
  if (!entry) return null;
  return now - entry.ts <= maxAgeMs ? entry : null;
}

/** Subscribe to cache writes; returns an unsubscribe function. */
export function subscribeScreenshots(fn: () => void): () => void {
  subscribers.add(fn);
  return () => {
    subscribers.delete(fn);
  };
}

/** Test helper: drop everything the cache holds. */
export function clearScreenshotCache(): void {
  cache.clear();
  notify();
}
