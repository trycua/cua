// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { dropTargetsOf, windowNumberOf, WindowDropTargets, type DropWindow, type Rect } from "../src/window-drop";

function fakeWindow(o: { bounds: Rect; number?: number | null; zoom?: number }) {
  const sent: [string, unknown][] = [];
  const state = { gone: false, shown: true };
  const w = {
    sent,
    get gone() {
      return state.gone;
    },
    set gone(v: boolean) {
      state.gone = v;
    },
    get shown() {
      return state.shown;
    },
    set shown(v: boolean) {
      state.shown = v;
    },
    destroyed: () => state.gone,
    visible: () => state.shown,
    contentBounds: () => o.bounds,
    zoom: () => o.zoom ?? 1,
    number: () => (o.number === undefined ? 900 : o.number),
    send: (event: string, payload: unknown) => void sent.push([event, payload]),
  } satisfies DropWindow & Record<string, unknown>;
  return w;
}

// The main window's page at (200, 100) on screen; a tile at 20..252 x 60..240 in it.
const TILES = [
  { spaceId: "local:aurora", x: 20, y: 60, width: 232, height: 180 },
  { spaceId: "cloud:build", x: 268, y: 60, width: 232, height: 180 },
];

describe("window drops onto the app's Space tiles", () => {
  it("finds the tile under a global point, in the page's CSS pixels at its zoom, for Spaces that take a drop", () => {
    const drops = new WindowDropTargets((id) => id !== "cloud:build");
    const main = fakeWindow({ bounds: { x: 200, y: 100, width: 1000, height: 700 } });
    drops.set(main, TILES);
    expect(drops.at(200 + 30, 100 + 70)).toEqual({ spaceId: "local:aurora", window: 900 });
    expect(drops.at(200 + 10, 100 + 70)).toBeNull();
    // A Space that takes no drop (the core's rule) is no target.
    expect(drops.at(200 + 300, 100 + 70)).toBeNull();
    // Outside the window.
    expect(drops.at(100, 100)).toBeNull();
    // Zoomed to 125%: CSS pixel 30 is 37.5 points in.
    const zoomed = new WindowDropTargets(() => true);
    zoomed.set(fakeWindow({ bounds: { x: 0, y: 0, width: 1000, height: 700 }, zoom: 1.25 }), TILES);
    expect(zoomed.at(20 * 1.25 + 1, 60 * 1.25 + 1)?.spaceId).toBe("local:aurora");
    expect(zoomed.at(21, 61)).toBeNull();
  });

  it("ignores hidden, minimized and closed windows, and a page that shows no tiles", () => {
    const drops = new WindowDropTargets(() => true);
    const main = fakeWindow({ bounds: { x: 0, y: 0, width: 1000, height: 700 } });
    drops.set(main, TILES);
    main.shown = false;
    expect(drops.at(30, 70)).toBeNull();
    main.shown = true;
    expect(drops.at(30, 70)?.spaceId).toBe("local:aurora");
    drops.set(main, []);
    expect(drops.at(30, 70)).toBeNull();
    drops.set(main, TILES);
    main.gone = true;
    expect(drops.at(30, 70)).toBeNull();
    // A window whose number is unknown can't be checked for cover: never a target.
    const unknown = new WindowDropTargets(() => true);
    unknown.set(fakeWindow({ bounds: { x: 0, y: 0, width: 1000, height: 700 }, number: null }), TILES);
    expect(unknown.at(30, 70)).toBeNull();
  });

  it("highlights the tile in its page and clears it there", () => {
    const drops = new WindowDropTargets(() => true);
    const main = fakeWindow({ bounds: { x: 0, y: 0, width: 1000, height: 700 } });
    drops.set(main, TILES);
    drops.highlight("local:aurora");
    drops.highlight("cloud:build");
    drops.highlight(null);
    expect(main.sent).toEqual([
      ["window.dropTarget", { spaceId: "local:aurora" }],
      ["window.dropTarget", { spaceId: null }],
      ["window.dropTarget", { spaceId: "cloud:build" }],
      ["window.dropTarget", { spaceId: null }],
    ]);
  });

  it("checks a page's report and reads the window number", () => {
    expect(dropTargetsOf([{ spaceId: "a", x: 1, y: 2, width: 3, height: 4, more: true }])).toEqual([{ spaceId: "a", x: 1, y: 2, width: 3, height: 4 }]);
    expect(dropTargetsOf([{ spaceId: "a", x: 1, y: 2, width: 3 }])).toBeNull();
    expect(dropTargetsOf([{ x: 1, y: 2, width: 3, height: 4 }])).toBeNull();
    expect(dropTargetsOf("nope")).toBeNull();
    expect(windowNumberOf("window:4242:0")).toBe(4242);
    expect(windowNumberOf("screen:1:0")).toBeNull();
    expect(windowNumberOf("window:0:0")).toBeNull();
  });
});
