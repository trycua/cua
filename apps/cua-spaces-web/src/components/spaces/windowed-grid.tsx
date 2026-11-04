// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Spaces grid, drawing only the rows on screen once it is long
 * (`WINDOW_FROM` tiles): with thousands of Spaces, select-all and each
 * filter keystroke redrew them all. Shorter grids are drawn whole, as
 * before. The columns are the browser's own (`auto-fill, minmax`), counted
 * from the grid's width so the rows here are the rows it lays out.
 *
 * `scrollToIndex` brings one tile into view even when it is not drawn yet.
 */

import { forwardRef, useCallback, useImperativeHandle, useLayoutEffect, useState, type ReactNode } from "react";

import { useWindowedRows } from "@/hooks/use-windowed-rows";
import { columnsFor, columnWidth, estimateTileRow, itemRange, rowOf, WINDOW_FROM } from "@/lib/windowing";

const MIN_COLUMN = 232;
const GAP_X = 20;
const GAP_Y = 28;

export interface WindowedGridHandle {
  /** Scrolls so the tile at `index` (counting the leading tiles first) is on screen. */
  scrollToIndex(index: number): void;
}

export const WindowedGrid = forwardRef<WindowedGridHandle, { count: number; renderAt: (index: number) => ReactNode; picking?: boolean }>(function WindowedGrid(
  { count, renderAt, picking },
  handle,
) {
  const [width, setWidth] = useState(0);
  const columns = columnsFor(width, MIN_COLUMN, GAP_X);
  const enabled = count >= WINDOW_FROM && width > 0;
  const rows = Math.ceil(count / columns);
  const { ref, window: win, style, scrollToRow } = useWindowedRows({
    enabled,
    rows,
    perRow: columns,
    gap: GAP_Y,
    estimatePitch: () => estimateTileRow(columnWidth(width, columns, GAP_X)) + GAP_Y,
  });

  // The grid's own width, for the columns (the element has no horizontal padding).
  const measure = useCallback(
    (el: HTMLUListElement | null) => {
      ref.current = el;
    },
    [ref],
  );
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    setWidth(el.clientWidth);
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(() => setWidth(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);

  useImperativeHandle(handle, () => ({ scrollToIndex: (index) => scrollToRow(rowOf(index, columns)) }), [scrollToRow, columns]);

  // Before the width is known (the first render) a long grid draws one screen, not all of it.
  const unmeasured = count >= WINDOW_FROM && width === 0;
  const [from, to] = enabled ? itemRange(win.start, win.end, columns, count) : unmeasured ? [0, 24] : [0, count];
  const tiles: ReactNode[] = [];
  for (let i = from; i < to; i++) tiles.push(renderAt(i));
  return (
    <ul
      ref={measure}
      style={style}
      data-picking={picking || undefined}
      data-windowed={enabled ? `${from}-${to}` : undefined}
      className="group/grid grid grid-cols-[repeat(auto-fill,minmax(232px,1fr))] gap-x-5 gap-y-7"
    >
      {tiles}
    </ul>
  );
});
