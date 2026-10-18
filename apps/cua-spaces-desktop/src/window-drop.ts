// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A real window dragged onto a Space tile in the app's own window (the
// SwiftUI app's `SidebarDropTargets`, which does it for its sidebar rows).
// The page reports where its tiles are (`window.setDropTargets`: CSS pixels
// from the top left of the page); the notch model follows the drag with the
// SDK's window-drag monitor and asks here which tile is under the cursor
// (`WindowTargets`), highlights it (`window.dropTarget` to that page), and
// commits a release there to the Teleport review. Which Spaces take a drop
// is the core's rule (`appSpaceAcceptsDrop`, the notch tiles' too). macOS
// only: window drags exist nowhere else (the monitor is the notch's). No
// Electron here (main.ts passes the windows in), so it is tested plainly.

import type { WindowTargetHit, WindowTargets } from "./notch";

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** A tile as the page reports it. */
export interface DropTargetRect extends Rect {
  spaceId: string;
}

/** The event that tells a page which tile to highlight. */
export const WINDOW_DROP_EVENT = "window.dropTarget";

/** One of the app's windows, as the drop targets need it. */
export interface DropWindow {
  /** Whether it is gone (closed). */
  destroyed(): boolean;
  /** On screen: shown and not minimized. */
  visible(): boolean;
  /** Its page's area in global top-left points (`getContentBounds`). */
  contentBounds(): Rect;
  /** The page's zoom (CSS pixels to points). */
  zoom(): number;
  /** Its window number (`kCGWindowNumber`), null when unknown. */
  number(): number | null;
  /** A host event to its page. */
  send(event: string, payload: unknown): void;
}

/** A page's report, checked: tiles with a Space id and a box. */
export function dropTargetsOf(v: unknown): DropTargetRect[] | null {
  if (!Array.isArray(v)) return null;
  const ok = v.every(
    (t) =>
      !!t &&
      typeof t === "object" &&
      typeof (t as Record<string, unknown>).spaceId === "string" &&
      ["x", "y", "width", "height"].every((k) => Number.isFinite((t as Record<string, unknown>)[k])),
  );
  return ok ? (v as DropTargetRect[]).map(({ spaceId, x, y, width, height }) => ({ spaceId, x, y, width, height })) : null;
}

/** `window:<number>:0` (Electron's `getMediaSourceId` on macOS) as the window's number. */
export function windowNumberOf(mediaSourceId: string): number | null {
  const m = /^window:(\d+):/.exec(mediaSourceId);
  const n = m ? Number(m[1]) : NaN;
  return Number.isInteger(n) && n > 0 ? n : null;
}

const inside = (r: Rect, x: number, y: number) => x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;

export class WindowDropTargets implements WindowTargets {
  private readonly pages = new Map<DropWindow, DropTargetRect[]>();
  private highlighted: { win: DropWindow; spaceId: string } | null = null;

  /** `accepts`: whether a Space takes a drop (the core's rule). */
  constructor(private readonly accepts: (spaceId: string) => boolean) {}

  /** A page's tiles now (none: it shows none). */
  set(win: DropWindow, targets: DropTargetRect[]): void {
    if (targets.length === 0) this.pages.delete(win);
    else this.pages.set(win, targets);
  }

  /** The drop-target tile under a global top-left point, in a window on screen (cover is the caller's check). */
  at(x: number, y: number): WindowTargetHit | null {
    for (const [win, targets] of this.pages) {
      if (win.destroyed()) {
        this.pages.delete(win);
        continue;
      }
      if (!win.visible()) continue;
      const content = win.contentBounds();
      if (!inside(content, x, y)) continue;
      const zoom = win.zoom() || 1;
      const px = (x - content.x) / zoom;
      const py = (y - content.y) / zoom;
      const tile = targets.find((t) => inside(t, px, py) && this.accepts(t.spaceId));
      const number = win.number();
      // The first window with a tile there (the main window; viewers show none); the caller checks it is not covered.
      if (tile && number !== null) return { spaceId: tile.spaceId, window: number };
    }
    return null;
  }

  /** Highlights a tile in the page that has it (and clears the last one). */
  highlight(spaceId: string | null): void {
    const prev = this.highlighted;
    const win = spaceId === null ? undefined : [...this.pages].find(([, ts]) => ts.some((t) => t.spaceId === spaceId))?.[0];
    if (prev && (prev.win !== win || prev.spaceId !== spaceId) && !prev.win.destroyed()) {
      prev.win.send(WINDOW_DROP_EVENT, { spaceId: null });
    }
    this.highlighted = win && spaceId !== null ? { win, spaceId } : null;
    if (win && spaceId !== null && !win.destroyed()) win.send(WINDOW_DROP_EVENT, { spaceId });
  }
}
