// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Where a picture-in-picture panel goes and what it loads, without Electron
// (`pip.ts` draws it; test/pip.test.ts checks this). As the SwiftUI app's
// `StreamPiPController`: 480 pt wide, its shape locked to the stream
// (16:10 until the first frame says otherwise), on top of everything.
// Unlike the Swift panel, which always opens centred, it opens where the
// last one was left (another one at once cascades from there).
import type { PipSpec } from "./model/streams";

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** The panel's width when it opens (`StreamPiPController.defaultWidth`). */
export const PIP_DEFAULT_WIDTH = 480;
/** The aspect until the stream's first frame (`StreamPiPController.aspect`). */
export const PIP_DEFAULT_ASPECT = 16 / 10;
export const PIP_MIN_WIDTH = 200;
/** How far each panel opened beside another moves down and right. */
export const PIP_CASCADE = 28;

/** The stream's aspect, or 16:10 for a size not known yet. */
export function aspectOf(width: number, height: number): number {
  return width > 0 && height > 0 ? width / height : PIP_DEFAULT_ASPECT;
}

/** The page that draws the panel: the web UI's picture-in-picture view (`apps/cua-spaces-web/src/pip`). */
export function pipPath(spec: PipSpec): string {
  const q = new URLSearchParams({ space: spec.spaceId, name: spec.spaceName, title: spec.title, key: spec.key, os: spec.os });
  if (spec.source.kind === "window") {
    q.set("window", spec.source.window.id);
    q.set("epoch", String(spec.source.window.epoch));
  }
  return `/pip?${q.toString()}`;
}

/** Whether at least 100 x 60 of `r` is on one of `areas`. */
export function onScreen(r: Rect, areas: readonly Rect[]): boolean {
  return areas.some((a) => {
    const w = Math.min(a.x + a.width, r.x + r.width) - Math.max(a.x, r.x);
    const h = Math.min(a.y + a.height, r.y + r.height) - Math.max(a.y, r.y);
    return w >= 100 && h >= 60;
  });
}

/**
 * A new panel's frame: where the last one was left (its width), shaped to
 * `aspect`, moved down and right for each panel already open; centred on
 * `work` (the primary display's work area) when there is no saved place or
 * it is off every display.
 */
export function initialPipBounds(o: { saved: Rect | null; work: Rect; areas: readonly Rect[]; aspect: number; open: number }): Rect {
  const width = Math.round(Math.max(PIP_MIN_WIDTH, o.saved?.width ?? PIP_DEFAULT_WIDTH));
  const height = Math.round(width / o.aspect);
  const shift = o.open * PIP_CASCADE;
  if (o.saved && onScreen(o.saved, o.areas)) return { x: o.saved.x + shift, y: o.saved.y + shift, width, height };
  return {
    x: Math.round(o.work.x + (o.work.width - width) / 2) + shift,
    y: Math.round(o.work.y + (o.work.height - height) / 2) + shift,
    width,
    height,
  };
}

/** The frame after the stream's size became known: the same width, the stream's shape (`sizeWatch`). */
export function fitToStream(current: Rect, streamWidth: number, streamHeight: number): Rect {
  return { ...current, height: Math.round(current.width / aspectOf(streamWidth, streamHeight)) };
}
