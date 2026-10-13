// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The page side of native video slots
 * (apps/cua-spaces-macos/docs/native-video.md).
 *
 * On macOS the SwiftUI host draws live Space video itself, as native views
 * over the web view. The page only says where, and whether the slot can be
 * seen: each mounted slot (a Space tile, the Space viewer) reports its rect,
 * the part of it its scroll containers leave visible (`clip`), and whether
 * page UI covers it (`occluded`: a dialog, a menu, a toast), all in CSS px
 * relative to the web view's viewport, on the `cuaVideo` script message
 * handler. The host registers that handler unless native video is turned
 * off (`WebUINativeVideo` NO), so the handler's presence is the flag. No handler: the
 * slot renders its fallback and nothing streams.
 *
 * Page → host (fire and forget):
 *
 *   { type: "surfaces", update: SurfaceState[], remove: string[] }
 *   { type: "focus", surfaceId: string | null }    // give keys to a viewer, or take them back
 *
 * Host → page (`cua:event`):
 *
 *   { event: "video.surface", payload: { surfaceId, state: "connecting" | "live" | "failed", reason?, opening? } }
 *
 * `opening` (with `failed`): the host could not open a stream at all (its
 * `streamProvider` threw), not one that failed once open.
 *   { event: "video.focus", payload: { surfaceId: string | null } }
 *
 * The host opens the Space's stream through its existing path
 * (`LiveStreamSession(provider: backend.streamProvider(id:))`); the page
 * never sees a ticket or a frame.
 */

export interface SurfaceRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** A Space tile (low rate, small, view only) or the Space viewer (full rate, takes input). */
export type SurfaceTier = "tile" | "full";

export interface SurfaceState {
  surfaceId: string;
  spaceId: string;
  tier: SurfaceTier;
  /** Pointer, scroll and keys go to the Space (the viewer); a tile passes them to the page. */
  interactive: boolean;
  /** Where the video goes, inside the slot's border. */
  rect: SurfaceRect;
  /** The part of `rect` its scroll containers and the viewport leave visible; null when none is. */
  clip: SurfaceRect | null;
  radius: number;
  /** Page UI (a dialog, a menu, a toast) covers part of the slot: the host hides the video. */
  occluded: boolean;
  /** On screen at all: the page is visible, the slot is laid out and not clipped away. */
  visible: boolean;
}

export type StreamSurfaceMessage =
  | { type: "surfaces"; update: SurfaceState[]; remove: string[] }
  | { type: "focus"; surfaceId: string | null };

export type SurfacePhase = "connecting" | "live" | "failed";

export interface VideoHandler {
  postMessage(message: StreamSurfaceMessage): void;
}

interface VideoHostWindow {
  webkit?: { messageHandlers?: { cuaVideo?: { postMessage(message: unknown): unknown } } };
}

/** The host's `cuaVideo` handler, or null when the host draws no native video. */
export function nativeVideoHandler(win: unknown = globalThis.window): VideoHandler | null {
  const handler = (win as VideoHostWindow | undefined)?.webkit?.messageHandlers?.cuaVideo;
  return handler && typeof handler.postMessage === "function" ? (handler as VideoHandler) : null;
}
