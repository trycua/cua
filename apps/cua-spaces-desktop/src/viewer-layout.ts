// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Where a Space's viewer window goes and what it loads, without Electron
// (`viewer.ts` draws it; test/viewer.test.ts checks this). As the SwiftUI
// app's `SpaceWindowView` ("Open in window"): the Space's desktop under a
// toolbar, at least 640 x 400, the window shaped to the stream once its
// size is known. Sizes here are the window's content (`useContentSize`);
// positions are its frame's. It opens where the last viewer was left, with
// that width (another one at once cascades from there).
import { onScreen, type Rect } from "./pip-layout";

/** The page's toolbar (source, size, Pop out): 44 px, as the Swift controls' bar. */
export const VIEWER_TOOLBAR = 44;
/** The content's width when nothing was saved. */
export const VIEWER_DEFAULT_WIDTH = 960;
/** `SpaceWindowView`'s minimum frame. */
export const VIEWER_MIN_WIDTH = 640;
export const VIEWER_MIN_HEIGHT = 400;
/** The stream's shape until its first frame. */
export const VIEWER_DEFAULT_ASPECT = 16 / 10;
/** How far each viewer opened beside another moves down and right. */
export const VIEWER_CASCADE = 28;
/** The share of the work area a viewer may take when it takes the stream's shape. */
const MAX_SHARE = 0.9;

/** Where the last viewer was left: its frame's position, its content's width. */
export interface ViewerPlace {
  x: number;
  y: number;
  width: number;
}

/** The page that draws the viewer: the web UI's viewer (`apps/cua-spaces-web/src/components/viewer`). */
export function viewerPath(spaceId: string, name: string, os?: string): string {
  const q = new URLSearchParams({ space: spaceId, name });
  if (os) q.set("os", os);
  return `/viewer?${q.toString()}`;
}

/** The content height for `width` at the stream's `aspect`, under the toolbar. */
const heightFor = (width: number, aspect: number) => Math.max(VIEWER_MIN_HEIGHT, Math.round(width / aspect) + VIEWER_TOOLBAR);

/**
 * A new viewer's frame position and content size: where the last one was
 * left (its width), 16:10 under the toolbar until the stream says its
 * shape, moved down and right for each viewer already open; centred on
 * `work` (the primary display's work area), at the saved width, when there
 * is no saved place or it is off every display. Never wider or taller than
 * the work area.
 */
export function initialViewerBounds(o: { saved: ViewerPlace | null; work: Rect; areas: readonly Rect[]; open: number }): Rect {
  const width = Math.round(Math.min(o.work.width, Math.max(VIEWER_MIN_WIDTH, o.saved?.width ?? VIEWER_DEFAULT_WIDTH)));
  const saved = o.saved && onScreen({ ...o.saved, width, height: VIEWER_MIN_HEIGHT }, o.areas) ? o.saved : null;
  const height = Math.min(o.work.height, heightFor(width, VIEWER_DEFAULT_ASPECT));
  const shift = o.open * VIEWER_CASCADE;
  if (saved) return { x: saved.x + shift, y: saved.y + shift, width, height };
  return {
    x: Math.round(o.work.x + (o.work.width - width) / 2) + shift,
    y: Math.round(o.work.y + (o.work.height - height) / 2) + shift,
    width,
    height,
  };
}

/**
 * The content size once the stream's size is known: the same width, the
 * stream's shape under the toolbar; narrower when that would be taller than
 * most of the work area. Never under the minimum.
 */
export function fitViewerToStream(content: { width: number; height: number }, stream: { width: number; height: number }, work: Rect): { width: number; height: number } {
  if (stream.width <= 0 || stream.height <= 0) return content;
  const aspect = stream.width / stream.height;
  let width = content.width;
  const tallest = Math.floor(work.height * MAX_SHARE);
  if (heightFor(width, aspect) > tallest) width = Math.floor((tallest - VIEWER_TOOLBAR) * aspect);
  width = Math.max(VIEWER_MIN_WIDTH, Math.min(width, work.width));
  // At the minimum width a tall stream is letterboxed rather than taller than the screen.
  return { width, height: Math.min(heightFor(width, aspect), Math.max(VIEWER_MIN_HEIGHT, tallest)) };
}
