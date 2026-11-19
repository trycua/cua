// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import {
  applyDragTrigger,
  displayFor,
  fallbackDragDisplays,
  initialDragTrigger,
  isResize,
  portalDragDisplays,
} from "./dragTrigger";

const mbp14 = {
  frame: { x: 0, y: 0, width: 1512, height: 982 },
  visibleFrame: { x: 0, y: 0, width: 1512, height: 945 },
  safeAreaTop: 32,
  auxLeftWidth: 662,
  auxRightWidth: 662,
};

describe("drag trigger (app core)", () => {
  it("tells a move from an edge resize", () => {
    const start = { x: 400, y: 300, width: 800, height: 600 };
    expect(isResize({ startFrame: start, frame: { ...start, x: 380, width: 820 } })).toBe(true);
    expect(isResize({ startFrame: start, frame: { ...start, x: 440, width: 802 } })).toBe(false);
    expect(isResize({})).toBe(false);
  });

  it("measures the line from the notch's bottom edge and picks the portal's display", () => {
    const d = portalDragDisplays([mbp14])[0]!;
    expect(d.notch).toEqual({ x: 660, y: 0, width: 192, height: 32 });
    expect(d.expanded).toEqual({ x: 376, y: 0, width: 760, height: 320 });
    const displays = fallbackDragDisplays({ x: 1512, y: 0, width: 1920, height: 1080 });
    expect(displays[0]!.notch.height).toBe(25);
    expect(displayFor([d, ...displays], { x: 1512, y: 0, width: 1920, height: 1080 })).toEqual(displays);

    let s = initialDragTrigger();
    const start = { x: 400, y: 300, width: 800, height: 600 };
    let t = applyDragTrigger(
      s,
      {
        type: "start",
        windowId: 1,
        appName: "Chrome",
        x: 700,
        y: 500,
        tMs: 0,
        startFrame: start,
        frame: { ...start, x: 440 },
      },
      [d],
    );
    expect(t.overlay.map((o) => o.type)).toEqual(["start"]);
    s = t.state;
    // 27 is the line (32 - 5): not above it.
    t = applyDragTrigger(s, { type: "cursor", x: 700, y: 27, tMs: 10 }, [d]);
    expect(t.state.phase).toBe("prompt");
    expect(t.tickAtMs).toBe(310);
    t = applyDragTrigger(t.state, { type: "cursor", x: 700, y: 26, tMs: 20 }, [d]);
    expect(t.state.phase).toBe("expanded");
    expect(t.overlay.map((o) => o.type)).toEqual(["enter-notch"]);
  });
});
