// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The window's drag regions for the SwiftUI host (`window.setDragRegions`).
 *
 * Electron reads `app-region: drag` from the CSS (`.app-drag` and
 * `.app-no-drag` in index.css). WebKit doesn't, so the Mac window drags
 * wherever it is told: until the page says, that is the whole top strip,
 * buttons included. The page sends the `.app-drag` boxes minus the controls
 * inside them and every `.app-no-drag` box (CSS pixels, from the top left),
 * again whenever the layout changes.
 */

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

/** What drags, and what stays clickable inside it (index.css). */
export const DRAG_SELECTOR = ".app-drag";
export const NO_DRAG_SELECTOR =
  '.app-no-drag, .app-drag :is(button, a, input, select, textarea, [role="button"], [data-no-drag]), [role="dialog"], [role="menu"], [role="listbox"]';

const area = (r: Rect) => r.width * r.height;

/** `r` minus `hole`: up to four boxes (above, below, left, right). */
export function subtract(r: Rect, hole: Rect): Rect[] {
  const left = Math.max(r.x, hole.x);
  const top = Math.max(r.y, hole.y);
  const right = Math.min(r.x + r.width, hole.x + hole.width);
  const bottom = Math.min(r.y + r.height, hole.y + hole.height);
  if (right <= left || bottom <= top) return [r];
  return [
    { x: r.x, y: r.y, width: r.width, height: top - r.y },
    { x: r.x, y: bottom, width: r.width, height: r.y + r.height - bottom },
    { x: r.x, y: top, width: left - r.x, height: bottom - top },
    { x: right, y: top, width: r.x + r.width - right, height: bottom - top },
  ].filter((p) => area(p) > 0);
}

/** The drag boxes with every hole cut out, rounded to whole pixels. */
export function dragRegions(drag: Rect[], holes: Rect[]): Rect[] {
  let out = drag.filter((r) => area(r) > 0);
  for (const hole of holes) {
    if (area(hole) <= 0) continue;
    out = out.flatMap((r) => subtract(r, hole));
  }
  return out.map((r) => ({ x: Math.round(r.x), y: Math.round(r.y), width: Math.round(r.width), height: Math.round(r.height) }));
}

const box = (el: Element): Rect => {
  const b = el.getBoundingClientRect();
  return { x: b.left, y: b.top, width: b.width, height: b.height };
};

/** The page's drag regions now. */
export function measureDragRegions(doc: Document): Rect[] {
  return dragRegions([...doc.querySelectorAll(DRAG_SELECTOR)].map(box), [...doc.querySelectorAll(NO_DRAG_SELECTOR)].map(box));
}

/**
 * Sends the drag regions now and after every layout change (resize, DOM
 * and class changes, once per frame at most, only when they differ).
 * Returns the stop function.
 */
export function syncDragRegions(win: Window, send: (rects: Rect[]) => void): () => void {
  const doc = win.document;
  let last = "";
  let frame: number | null = null;
  const flush = () => {
    frame = null;
    const rects = measureDragRegions(doc);
    const key = JSON.stringify(rects);
    if (key === last) return;
    last = key;
    send(rects);
  };
  const schedule = () => {
    if (frame === null) frame = win.requestAnimationFrame(flush);
  };
  const observer = new MutationObserver(schedule);
  observer.observe(doc.documentElement, {
    subtree: true,
    childList: true,
    attributes: true,
    attributeFilter: ["class", "style", "hidden", "data-state", "open"],
  });
  win.addEventListener("resize", schedule);
  // Transitions (the sidebar sliding) end after the last mutation.
  doc.addEventListener("transitionend", schedule, true);
  schedule();
  return () => {
    observer.disconnect();
    win.removeEventListener("resize", schedule);
    doc.removeEventListener("transitionend", schedule, true);
    if (frame !== null) win.cancelAnimationFrame(frame);
  };
}
