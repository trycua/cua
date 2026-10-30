// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Drawing only what is on screen, for the Spaces page's grid and list: with
 * thousands of Spaces every select-all or filter keystroke redrew thousands
 * of tiles. The maths is here (no DOM) so it is tested without a browser;
 * `hooks/use-windowed-rows.ts` feeds it the scroll position.
 *
 * Rows are the unit: a grid row holds `columns` tiles, a list row one. Every
 * row has the same pitch (its height plus the gap below it), measured from
 * what is drawn, so the rows above and below the window are two spacers.
 */

/** Lists shorter than this are drawn whole: nothing to gain, and the page
 * keeps its plain layout (and its tests, which have no layout engine). */
export const WINDOW_FROM = 100;

/** Rows drawn beyond each edge of the viewport (so a scroll or a Tab has
 * something there before the next frame draws more). */
export const OVERSCAN_ROWS = 3;

export interface WindowInput {
  /** Rows in all. */
  rows: number;
  /** One row's height plus the gap below it (px). */
  pitch: number;
  /** The scroll container's scrollTop (px). */
  scrollTop: number;
  /** The scroll container's visible height (px). */
  viewport: number;
  /** Where the first row starts inside the scroll container's content (px). */
  offset: number;
  overscan?: number;
}

export interface RowWindow {
  /** First row drawn. */
  start: number;
  /** One past the last row drawn. */
  end: number;
  /** Height standing in for the rows before `start` (px). */
  before: number;
  /** Height standing in for the rows from `end` on (px). */
  after: number;
}

/** The rows to draw for a scroll position. */
export function visibleRows({ rows, pitch, scrollTop, viewport, offset, overscan = OVERSCAN_ROWS }: WindowInput): RowWindow {
  if (rows <= 0 || pitch <= 0) return { start: 0, end: Math.max(0, rows), before: 0, after: 0 };
  const top = scrollTop - offset;
  const first = Math.floor(top / pitch);
  const last = Math.ceil((top + viewport) / pitch);
  const start = Math.min(rows, Math.max(0, first - overscan));
  const end = Math.min(rows, Math.max(start, last + overscan));
  return { start, end, before: start * pitch, after: (rows - end) * pitch };
}

/** Columns of a grid of `auto-fill, minmax(min, 1fr)` columns `width` wide
 * with `gap` between them: what the browser lays out, so the rows computed
 * here are the rows it draws. */
export function columnsFor(width: number, min: number, gap: number): number {
  if (width <= 0) return 1;
  return Math.max(1, Math.floor((width + gap) / (min + gap)));
}

/** The row an item is in. */
export function rowOf(index: number, columns: number): number {
  return Math.floor(index / Math.max(1, columns));
}

/** The scrollTop that brings `row` into view with the least movement (the
 * current one when it already is), or `null` when it already is. */
export function rowScrollTop({
  row,
  pitch,
  gap,
  scrollTop,
  viewport,
  offset,
}: {
  row: number;
  pitch: number;
  /** The gap below a row, which is not part of the row itself. */
  gap: number;
  scrollTop: number;
  viewport: number;
  offset: number;
}): number | null {
  const top = offset + row * pitch;
  const bottom = top + pitch - gap;
  if (top >= scrollTop && bottom <= scrollTop + viewport) return null;
  return top < scrollTop ? top : bottom - viewport;
}

/** The first and last item indexes of rows `start` to `end` (exclusive) of
 * `count` items in `columns` columns. */
export function itemRange(start: number, end: number, columns: number, count: number): [number, number] {
  return [Math.min(count, start * columns), Math.min(count, end * columns)];
}

/** An estimate of a grid row's height before one is drawn: the 16:10
 * picture plus the name and place lines under it (`SpaceTile`). */
export function estimateTileRow(columnWidth: number): number {
  return Math.round(columnWidth * 0.625) + 12 + 20 + 16;
}

/** The column width of a grid `width` wide in `columns` columns. */
export function columnWidth(width: number, columns: number, gap: number): number {
  return (width - gap * (columns - 1)) / columns;
}
