// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Which rows of a long grid or list to draw (`lib/windowing.ts` has the
 * maths). The element `ref` goes on is the one holding the rows: the hook
 * keeps the room of the rows before and after the window as its own top and
 * bottom padding (`style`), so the scrollbar is the full list's and the rows
 * drawn sit where they would with all of them there.
 *
 * It follows the nearest scrolling ancestor and the element's own width, and
 * measures the real row pitch from the rows it drew. Off (`enabled` false) it
 * draws every row and touches nothing.
 */

import { useCallback, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type RefObject } from "react";

import { rowScrollTop, visibleRows, type RowWindow } from "@/lib/windowing";

export interface WindowedRows {
  ref: RefObject<HTMLElement | null>;
  window: RowWindow;
  /** Padding that stands in for the rows not drawn; spread it on the element `ref` is on. */
  style: CSSProperties;
  /** Scrolls so `row` is on screen (a row that is not drawn yet draws after the scroll). */
  scrollToRow(row: number): void;
}

function scrollParent(el: HTMLElement): HTMLElement {
  for (let p = el.parentElement; p; p = p.parentElement) {
    const o = getComputedStyle(p).overflowY;
    if (o === "auto" || o === "scroll") return p;
  }
  return (document.scrollingElement as HTMLElement | null) ?? document.documentElement;
}

export function useWindowedRows({
  enabled,
  rows,
  perRow,
  estimatePitch,
  gap,
}: {
  enabled: boolean;
  rows: number;
  /** Items in a row (columns; 1 for a list). */
  perRow: number;
  /** A row's pitch before any row is drawn (px). */
  estimatePitch: () => number;
  /** The gap between rows (px). */
  gap: number;
}): WindowedRows {
  const ref = useRef<HTMLElement | null>(null);
  const pitch = useRef(0);
  const estimate = useRef(estimatePitch);
  estimate.current = estimatePitch;
  const [win, setWin] = useState<RowWindow>({ start: 0, end: Math.min(rows, 24), before: 0, after: 0 });

  const read = useCallback(() => {
    const el = ref.current;
    if (!el) return null;
    const scroller = scrollParent(el);
    const scrollTop = scroller.scrollTop;
    const offset = el.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scrollTop;
    return { el, scroller, scrollTop, offset, viewport: scroller.clientHeight };
  }, []);

  const update = useCallback(() => {
    const r = read();
    if (!r) return;
    // The real pitch: the distance between the tops of the first two rows drawn.
    const kids = r.el.children;
    if (kids.length > perRow) {
      const d = (kids[perRow] as HTMLElement).offsetTop - (kids[0] as HTMLElement).offsetTop;
      if (d > 0) pitch.current = d;
    }
    const p = pitch.current || estimate.current();
    const next = visibleRows({ rows, pitch: p, scrollTop: r.scrollTop, viewport: r.viewport || 800, offset: r.offset });
    setWin((cur) => (cur.start === next.start && cur.end === next.end && cur.before === next.before && cur.after === next.after ? cur : next));
  }, [read, rows, perRow]);

  // Follow the scroll (one update per frame) and the sizes of what scrolls and what is scrolled.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!enabled || !el) return;
    const scroller = scrollParent(el);
    let frame = 0;
    const schedule = () => {
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        update();
      });
    };
    update();
    scroller.addEventListener("scroll", schedule, { passive: true });
    const ro = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(schedule);
    ro?.observe(scroller);
    ro?.observe(el);
    return () => {
      scroller.removeEventListener("scroll", schedule);
      ro?.disconnect();
      if (frame) cancelAnimationFrame(frame);
    };
  }, [enabled, update]);

  // A window just drawn gives a better pitch than the estimate.
  useLayoutEffect(() => {
    if (enabled) update();
  }, [enabled, win.start, win.end, update]);

  const scrollToRow = useCallback(
    (row: number) => {
      const r = read();
      if (!r) return;
      const to = rowScrollTop({ row, pitch: pitch.current || estimate.current(), gap, scrollTop: r.scrollTop, viewport: r.viewport, offset: r.offset });
      if (to !== null) r.scroller.scrollTo({ top: Math.max(0, to) });
    },
    [read, gap],
  );

  const shown = useMemo<RowWindow>(() => (enabled ? win : { start: 0, end: rows, before: 0, after: 0 }), [enabled, win, rows]);
  const style = useMemo<CSSProperties>(() => (enabled ? { paddingTop: shown.before, paddingBottom: shown.after } : {}), [enabled, shown.before, shown.after]);
  return { ref, window: shown, style, scrollToRow };
}
