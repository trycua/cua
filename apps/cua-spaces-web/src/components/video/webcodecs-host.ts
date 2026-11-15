// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { detectMode, type CuaDesktopBridge, type HostWindow } from "@/bridge/detect";
import { nativeVideoHandler } from "@/lib/stream-surface";

/** `SlotStatus.reason` when the shell has no video for this page at all (no cua here, or sample data). */
export const NO_VIDEO_HERE = "unsupported";

/**
 * The Electron shell when the page should decode a Space's video itself
 * (WebCodecs into a canvas): the shell is there, Chromium has
 * `VideoDecoder`, and no host draws native video instead (the Mac app's
 * `cuaVideo`). Null anywhere else. Small on purpose: the decoder and the
 * production `MediaSession` load only when a slot needs them
 * (`webcodecs-slots.ts`).
 */
export function webCodecsHost(win: unknown = globalThis.window): CuaDesktopBridge | null {
  const w = win as (HostWindow & { VideoDecoder?: unknown }) | undefined;
  // Also null under ?bridge=demo: sample Spaces have no video.
  if (!w?.cuaDesktop || detectMode(w) !== "electron") return null;
  if (typeof w.VideoDecoder !== "function") return null;
  if (nativeVideoHandler(win) !== null) return null;
  return w.cuaDesktop;
}
