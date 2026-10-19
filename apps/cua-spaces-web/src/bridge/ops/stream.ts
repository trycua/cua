// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Live video for hosts that let the page draw it (WebCodecs): a media ticket
 * for a Space's desktop. The page attaches the production `MediaSession`
 * (libs/cua/crates/cua-spacesd-html5/web/src/core/mediaSession.ts) to the
 * URL and decodes into a canvas, so the video never passes through the
 * bridge (`components/video/webcodecs-slot.ts`).
 *
 * | Operation | Electron | SwiftUI | Tauri |
 * |---|---|---|---|
 * | `spaces.openStream {spaceId, tier}` | the host method `spaces.openStream`: the daemon's `OpenMediaBridge` (`src/media-bridge.ts`) | unsupported: it draws video natively (`cuaVideo`) | unsupported: its viewer is a window of its own |
 *
 * `tier` is `tile` (a Space tile: 10 fps, 960 px long edge, view only) or
 * `full` (the viewer: the Space's defaults, with input), the macOS app's
 * `VideoTier`. `windowId` (with its `epoch`) streams one window instead of
 * the desktop (picture in picture): its input goes in the background, as
 * the Swift app's `SpaceStreamProvider.inputPolicy`, unless `activate` (the
 * Space refused that window's input without activating it). The demo host
 * has no video and answers unsupported, so the page keeps its fallback (the
 * drawn thumbnail, Open window).
 */

import { UnsupportedOperationError, type BridgeMode } from "../adapter";
import type { OpCoverage } from "../coverage";

export type StreamTier = "tile" | "full";

/** Where to attach: a ticketed `ws://…` URL speaking rcdp wire v2. */
export interface StreamTicket {
  wsUrl: string;
  /** ISO time after which the ticket no longer attaches; null when the host doesn't say. */
  expiresAt: string | null;
}

export interface StreamTarget {
  spaceId: string;
  tier: StreamTier;
  windowId?: string;
  epoch?: number;
  activate?: boolean;
}

export interface StreamOps {
  "spaces.openStream": { args: StreamTarget; result: StreamTicket };
}

export const STREAM_OPERATIONS = ["spaces.openStream"] as const satisfies readonly (keyof StreamOps)[];

export const STREAM_COVERAGE = {
  "spaces.openStream": {
    webkit: { methods: [] },
    electron: { methods: ["spaces.openStream"] },
    tauri: [],
    unsupported: {
      webkit: "the Mac app draws video natively over the page (cuaVideo), so the page never asks for a ticket",
      tauri: "the Tauri app plays a Space's video in its own viewer window",
    },
  },
} as const satisfies Record<keyof StreamOps, OpCoverage>;

/** Hosts that draw no video in the page. */
export function unsupportedStreamOps(mode: BridgeMode) {
  return {
    "spaces.openStream": (): Promise<StreamTicket> => Promise.reject(new UnsupportedOperationError(mode, "spaces.openStream")),
  };
}

/** Arguments that take it down its usual path (coverage tests). */
export const STREAM_ARGS = { "spaces.openStream": { spaceId: "local:design-review", tier: "tile" as const } };
