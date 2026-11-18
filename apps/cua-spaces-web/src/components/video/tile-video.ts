// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { RefObject } from "react";

import type { Space } from "@/bridge";
import { spaceState } from "@/lib/spaces";

import { useVideoSlot, type VideoSlot } from "./use-video-slot";

/** A tile's radius (`rounded-lg`), for the native layer's clip. */
const TILE_RADIUS = 8;

/**
 * Whether a Space's tile can show its live desktop: it runs, and, when the
 * registry knows it, it answered and can stream its desktop.
 */
export function canStreamTile(space: Space): boolean {
  if (spaceState(space) !== "running") return false;
  // This Mac's own desktop is never streamed here: it would only mirror itself.
  if (space.id === "this-mac") return false;
  const sdk = space.sdk;
  return !sdk || (sdk.reachable && sdk.features.includes("desktop_stream"));
}

/**
 * A Space tile's live video: small, at a low frame rate, view only (clicks
 * and scrolls go to the page). Null where the host draws no video; the tile
 * keeps its drawn thumbnail under the video, so a stream that never starts
 * or fails leaves the thumbnail. `inView`: the tile is on screen or near it
 * (`useInView`); off it, no video.
 */
export function useTileVideo(ref: RefObject<HTMLElement | null>, space: Space, inView = true): VideoSlot | null {
  // Only tiles near the viewport stream: the host opens one live session per
  // slot, and a grid of 200 running Spaces would decode 200 desktops.
  return useVideoSlot(ref, { active: inView && canStreamTile(space), spaceId: space.id, tier: "tile", interactive: false, radius: TILE_RADIUS, os: space.os });
}
