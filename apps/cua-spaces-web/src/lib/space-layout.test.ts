// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import {
  clampFraction,
  clampSplit,
  decideDock,
  DEFAULT_H_SPLIT,
  DEFAULT_PREFS,
  DEFAULT_V_SPLIT,
  DOCK_MIN_WIDTH,
  effectiveDock,
  EDGE_ENTER,
  EDGE_EXIT,
  MIN_BELOW_H,
  MIN_BELOW_W,
  MIN_STAGE_H,
  MIN_STAGE_W,
  sanitizePrefs,
  splitOf,
  TOP_ENTER,
  TOP_EXIT,
} from "./space-layout";

describe("space layout rules", () => {
  it("keeps both panes at their minimum size, on either axis", () => {
    expect(clampSplit("top", 0.01, 900, 1000)).toBeCloseTo(MIN_STAGE_H / 1000);
    expect(clampSplit("top", 0.99, 900, 1000)).toBeCloseTo(1 - MIN_BELOW_H / 1000);
    expect(clampSplit("left", 0.01, 1000, 700)).toBeCloseTo(MIN_STAGE_W / 1000);
    expect(clampSplit("right", 0.99, 1000, 700)).toBeCloseTo(1 - MIN_BELOW_W / 1000);
    expect(clampSplit("top", 0.4, 900, 1000)).toBe(0.4);
  });

  it("halves a column too small for both minimums, and survives nonsense", () => {
    expect(clampFraction(0.9, 300, MIN_STAGE_H, MIN_BELOW_H)).toBe(0.5);
    expect(clampFraction(Number.NaN, 1000, 100, 100)).toBe(0.5);
  });

  it("falls back to the vertical layout in a narrow column, and says nothing", () => {
    expect(effectiveDock("left", DOCK_MIN_WIDTH - 1)).toBe("top");
    expect(effectiveDock("right", DOCK_MIN_WIDTH - 1)).toBe("top");
    expect(effectiveDock("left", DOCK_MIN_WIDTH)).toBe("left");
    expect(effectiveDock("top", 100)).toBe("top");
    // Not measured yet: the saved dock stands, so the first paint does not flip.
    expect(effectiveDock("right", 0)).toBe("right");
  });

  it("reads the size of the axis the dock splits", () => {
    const p = { vSplit: 0.4, hSplit: 0.7 };
    expect(splitOf(p, "top")).toBe(0.4);
    expect(splitOf(p, "left")).toBe(0.7);
    expect(splitOf(p, "right")).toBe(0.7);
  });

  describe("dock decision", () => {
    it("docks to the edge the pointer reaches, and nowhere in the middle", () => {
      expect(decideDock("top", "top", 0.5, 0.5)).toBe("top");
      expect(decideDock("top", "top", EDGE_ENTER - 0.01, 0.5)).toBe("left");
      expect(decideDock("top", "top", 1 - EDGE_ENTER + 0.01, 0.5)).toBe("right");
      expect(decideDock("top", "top", EDGE_ENTER + 0.01, 0.5)).toBe("top");
    });

    it("has hysteresis: a preview stays until the pointer leaves the wider zone", () => {
      expect(decideDock("top", "left", EDGE_ENTER + 0.05, 0.5)).toBe("left");
      expect(decideDock("top", "left", EDGE_EXIT + 0.01, 0.5)).toBe("top");
      expect(decideDock("top", "right", 1 - EDGE_ENTER - 0.05, 0.5)).toBe("right");
      expect(decideDock("top", "right", 1 - EDGE_EXIT - 0.01, 0.5)).toBe("top");
    });

    it("returns a docked desktop to the top only near the top, with hysteresis", () => {
      expect(decideDock("left", "left", 0.4, 0.5)).toBe("left");
      // The divider of a docked layout sits mid-width: the sides never re-dock.
      expect(decideDock("left", "left", 0.02, 0.5)).toBe("left");
      expect(decideDock("right", "right", 0.98, 0.5)).toBe("right");
      expect(decideDock("left", "left", 0.4, TOP_ENTER - 0.01)).toBe("top");
      expect(decideDock("left", "left", 0.4, TOP_ENTER + 0.02)).toBe("left");
      expect(decideDock("right", "top", 0.6, TOP_EXIT - 0.02)).toBe("top");
      expect(decideDock("right", "top", 0.6, TOP_EXIT + 0.02)).toBe("right");
    });
  });

  describe("saved prefs", () => {
    it("defaults for nothing, and for anything unreadable", () => {
      expect(sanitizePrefs(undefined)).toEqual(DEFAULT_PREFS);
      expect(sanitizePrefs("x")).toEqual(DEFAULT_PREFS);
      expect(sanitizePrefs({ dock: "bottom", vSplit: 3, hSplit: -1, folded: 7 })).toEqual(DEFAULT_PREFS);
      expect(DEFAULT_PREFS.vSplit).toBe(DEFAULT_V_SPLIT);
      expect(DEFAULT_PREFS.hSplit).toBe(DEFAULT_H_SPLIT);
    });

    it("keeps what is valid", () => {
      expect(sanitizePrefs({ dock: "right", vSplit: 0.3, hSplit: 0.6, folded: { stream: true, details: "yes" } })).toEqual({
        dock: "right",
        vSplit: 0.3,
        hSplit: 0.6,
        folded: { people: false, children: false, stream: true, details: false },
      });
    });
  });
});
