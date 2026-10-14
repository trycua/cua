// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A real window dragged onto a Space tile (the classic Mac sidebar's window
 * drop on a row, `SidebarDropTargets.swift`).
 *
 * The page can't see such a drag: another app's window moving over this one
 * sends the page no events. The host follows it with the SDK's window-drag
 * monitor (the one the notch follows; macOS, with the Accessibility
 * permission), so the page tells the host where its tiles are
 * (`window.setDropTargets`: each `[data-window-drop]` element's Space id and
 * visible box, in CSS pixels from the top left, again whenever the layout
 * changes). The host hit-tests the cursor against them, decides which Space
 * takes a drop (the core's rule, as for the notch's tiles), tells the page
 * which tile to highlight (`window.dropTarget`, `{ spaceId }` or null) and,
 * on release, opens that Space's Teleport review for the window's app.
 *
 * A host that can't follow window drags (Windows, Linux, the browser demo)
 * never sends `window.dropTarget`, so nothing shows there.
 */

import { useSyncExternalStore } from "react";
import type { Rect } from "./drag-regions";

/** The attribute that makes an element a window-drop target; its value is the Space id. */
export const WINDOW_DROP_ATTR = "data-window-drop";
/** The host's event naming the targeted Space (or null). */
export const WINDOW_DROP_EVENT = "window.dropTarget";

export interface DropTargetRect extends Rect {
  spaceId: string;
}

/** `a` clipped to `b`, or null when they don't overlap. */
export function clip(a: Rect, b: Rect): Rect | null {
  const left = Math.max(a.x, b.x);
  const top = Math.max(a.y, b.y);
  const right = Math.min(a.x + a.width, b.x + b.width);
  const bottom = Math.min(a.y + a.height, b.y + b.height);
  if (right <= left || bottom <= top) return null;
  return { x: left, y: top, width: right - left, height: bottom - top };
}

const box = (el: Element): Rect => {
  const b = el.getBoundingClientRect();
  return { x: b.left, y: b.top, width: b.width, height: b.height };
};

const clips = (style: CSSStyleDeclaration) => [style.overflow, style.overflowX, style.overflowY].some((o) => o && o !== "visible");

/**
 * The page's drop targets now: each tile's box clipped to the viewport and
 * to every scrolling ancestor (a tile scrolled half out of view takes a drop
 * on its visible half only). None while a dialog or menu covers the page.
 */
export function measureDropTargets(win: Window): DropTargetRect[] {
  const doc = win.document;
  if (doc.querySelector('[role="dialog"], [role="alertdialog"], [role="menu"]')) return [];
  const viewport: Rect = { x: 0, y: 0, width: win.innerWidth, height: win.innerHeight };
  const out: DropTargetRect[] = [];
  for (const el of doc.querySelectorAll(`[${WINDOW_DROP_ATTR}]`)) {
    const spaceId = el.getAttribute(WINDOW_DROP_ATTR);
    if (!spaceId) continue;
    let visible: Rect | null = clip(box(el), viewport);
    for (let p = el.parentElement; visible && p && p !== doc.documentElement; p = p.parentElement) {
      if (clips(win.getComputedStyle(p))) visible = clip(visible, box(p));
    }
    if (!visible) continue;
    out.push({
      spaceId,
      x: Math.round(visible.x),
      y: Math.round(visible.y),
      width: Math.round(visible.width),
      height: Math.round(visible.height),
    });
  }
  return out;
}

/**
 * Sends the drop targets now and after every layout change (resize, scroll,
 * DOM and class changes, once per frame at most, only when they differ).
 * Returns the stop function.
 */
export function syncDropTargets(win: Window, send: (targets: DropTargetRect[]) => void): () => void {
  const doc = win.document;
  let last = "";
  let frame: number | null = null;
  const flush = () => {
    frame = null;
    const targets = measureDropTargets(win);
    const key = JSON.stringify(targets);
    if (key === last) return;
    last = key;
    send(targets);
  };
  const schedule = () => {
    if (frame === null) frame = win.requestAnimationFrame(flush);
  };
  const observer = new MutationObserver(schedule);
  observer.observe(doc.documentElement, {
    subtree: true,
    childList: true,
    attributes: true,
    attributeFilter: ["class", "style", "hidden", "data-state", "open", "role", WINDOW_DROP_ATTR],
  });
  win.addEventListener("resize", schedule);
  doc.addEventListener("scroll", schedule, true);
  doc.addEventListener("transitionend", schedule, true);
  schedule();
  return () => {
    observer.disconnect();
    win.removeEventListener("resize", schedule);
    doc.removeEventListener("scroll", schedule, true);
    doc.removeEventListener("transitionend", schedule, true);
    if (frame !== null) win.cancelAnimationFrame(frame);
  };
}

/* ---- The targeted tile (the host's `window.dropTarget`) ------------------- */

let targeted: string | null = null;
const listeners = new Set<() => void>();

/** The host says which tile a dragged window is over (null: none). */
export function setWindowDropTarget(spaceId: string | null): void {
  if (spaceId === targeted) return;
  targeted = spaceId;
  for (const l of listeners) l();
}

/** The host's `window.dropTarget` payload, checked. */
export function windowDropTargetOf(payload: unknown): string | null {
  const id = (payload as { spaceId?: unknown } | null)?.spaceId;
  return typeof id === "string" && id ? id : null;
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

/** The Space a dragged window is over now, if any. */
export function useWindowDropTarget(): string | null {
  return useSyncExternalStore(subscribe, () => targeted, () => null);
}
