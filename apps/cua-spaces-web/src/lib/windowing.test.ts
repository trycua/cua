// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { columnsFor, columnWidth, estimateTileRow, itemRange, OVERSCAN_ROWS, rowOf, rowScrollTop, visibleRows, WINDOW_FROM } from "./windowing";

describe("visibleRows", () => {
  const base = { rows: 1000, pitch: 100, viewport: 500, offset: 200, overscan: 2 };

  it("draws the first screen and a little more at the top, the rest as spacers", () => {
    const w = visibleRows({ ...base, scrollTop: 0 });
    // The grid starts 200 px down; a 500 px viewport sees rows 0..3 (up to y=500): ceil((0-200+500)/100) = 3.
    expect(w).toEqual({ start: 0, end: 5, before: 0, after: 995 * 100 - 0 });
    expect(w.before + (w.end - w.start) * 100 + w.after).toBe(1000 * 100);
  });

  it("follows the scroll and keeps the overscan on both sides", () => {
    const w = visibleRows({ ...base, scrollTop: 20_200 });
    // 20 000 px into the grid is row 200; the viewport shows 200..205.
    expect(w.start).toBe(198);
    expect(w.end).toBe(207);
    expect(w.before).toBe(198 * 100);
    expect(w.after).toBe((1000 - 207) * 100);
  });

  it("ends at the last row and never past it", () => {
    const w = visibleRows({ ...base, scrollTop: 1000 * 100 });
    expect(w.end).toBe(1000);
    expect(w.after).toBe(0);
    expect(w.start).toBeLessThan(w.end);
    expect(w.start).toBeGreaterThan(900);
  });

  it("draws every row of a list that is not long, and nothing for an empty one", () => {
    expect(visibleRows({ ...base, rows: 0, scrollTop: 0 })).toEqual({ start: 0, end: 0, before: 0, after: 0 });
    expect(visibleRows({ ...base, rows: 4, scrollTop: 0 })).toMatchObject({ start: 0, end: 4, after: 0 });
  });

  it("does not divide by a pitch that is not known yet", () => {
    expect(visibleRows({ ...base, pitch: 0, scrollTop: 0 })).toEqual({ start: 0, end: 1000, before: 0, after: 0 });
  });

  it("uses an overscan by default", () => {
    const w = visibleRows({ rows: 1000, pitch: 100, viewport: 500, offset: 0, scrollTop: 10_000 });
    expect(w.start).toBe(100 - OVERSCAN_ROWS);
  });

  it("draws a bounded number of rows however long the list is (the point of it)", () => {
    for (const rows of [100, 1_000, 100_000]) {
      const w = visibleRows({ rows, pitch: 260, viewport: 900, offset: 150, scrollTop: 12_345 });
      expect(w.end - w.start).toBeLessThanOrEqual(Math.ceil(900 / 260) + 2 * OVERSCAN_ROWS + 1);
    }
  });
});

describe("columns and rows", () => {
  it("counts the columns the browser lays out for auto-fill minmax(232px, 1fr) with a 20 px gap", () => {
    expect(columnsFor(232, 232, 20)).toBe(1);
    expect(columnsFor(483, 232, 20)).toBe(1); // 2 columns need 232*2+20 = 484
    expect(columnsFor(484, 232, 20)).toBe(2);
    expect(columnsFor(1024, 232, 20)).toBe(4); // 4*232+3*20 = 988
    expect(columnsFor(0, 232, 20)).toBe(1);
  });

  it("gives an item its row and a grid its column width", () => {
    expect(rowOf(0, 4)).toBe(0);
    expect(rowOf(3, 4)).toBe(0);
    expect(rowOf(4, 4)).toBe(1);
    expect(rowOf(9, 0)).toBe(9);
    expect(columnWidth(988, 4, 20)).toBe(232);
  });

  it("estimates a tile row from its 16:10 picture and the two lines under it", () => {
    expect(estimateTileRow(232)).toBe(145 + 48);
  });

  it("turns rows into the items to draw, clamped to the list", () => {
    expect(itemRange(2, 5, 4, 100)).toEqual([8, 20]);
    expect(itemRange(24, 30, 4, 100)).toEqual([96, 100]);
    expect(itemRange(30, 40, 4, 100)).toEqual([100, 100]);
  });

  it("draws short lists whole", () => {
    expect(WINDOW_FROM).toBeGreaterThan(70); // the page's own tests have fewer Spaces than this
  });
});

describe("rowScrollTop", () => {
  const view = { pitch: 100, gap: 20, viewport: 500, offset: 200 };

  it("leaves a row that is on screen where it is", () => {
    expect(rowScrollTop({ ...view, row: 1, scrollTop: 0 })).toBeNull();
  });

  it("scrolls up to a row above, to its top", () => {
    expect(rowScrollTop({ ...view, row: 3, scrollTop: 2000 })).toBe(200 + 300);
  });

  it("scrolls down to a row below, to its bottom edge (not its gap)", () => {
    // Row 50 spans 5200..5280 (the gap below is not part of it); the viewport ends there.
    expect(rowScrollTop({ ...view, row: 50, scrollTop: 0 })).toBe(5200 + 80 - 500);
  });
});
