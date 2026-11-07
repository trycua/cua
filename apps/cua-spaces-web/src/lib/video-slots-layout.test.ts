// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { LAYOUT_CHANGE_EVENT, type StreamSurfaceMessage } from "./stream-surface";
import { VideoSlotReporter } from "./video-slots";

class FakeResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

function box(el: Element, r: { x: number; y: number; width: number; height: number }) {
  el.getBoundingClientRect = () => ({ ...r, top: r.y, left: r.x, right: r.x + r.width, bottom: r.y + r.height, toJSON: () => r }) as DOMRect;
}

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", FakeResizeObserver);
  vi.stubGlobal("innerWidth", 1400);
  vi.stubGlobal("innerHeight", 900);
  document.elementFromPoint = () => null;
  document.elementsFromPoint = () => [];
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("VideoSlotReporter after a layout change", () => {
  // The Space page's panes move (a divider dragged, the desktop docked to a side): the host must get the
  // new rect in the same frame, not on the next animation frame or the 400 ms poll.
  it("reports the new rect the moment the page says its layout changed", () => {
    const posted: StreamSurfaceMessage[] = [];
    const reporter = new VideoSlotReporter({ postMessage: (m) => posted.push(m) });
    const viewer = document.createElement("div");
    document.body.append(viewer);
    box(viewer, { x: 20, y: 60, width: 700, height: 360 });
    const stop = reporter.register(viewer, { spaceId: "a", tier: "full", interactive: true, radius: 8 }, () => {});
    const rects = () => posted.flatMap((m) => (m.type === "surfaces" ? m.update.map((u) => u.rect) : []));
    expect(rects().at(-1)).toEqual({ x: 20, y: 60, width: 700, height: 360 });

    // Docked to the left: tall and narrow.
    box(viewer, { x: 20, y: 60, width: 420, height: 760 });
    window.dispatchEvent(new Event(LAYOUT_CHANGE_EVENT));
    expect(rects().at(-1)).toEqual({ x: 20, y: 60, width: 420, height: 760 });

    // Docked to the right: the slot moved across.
    box(viewer, { x: 900, y: 60, width: 480, height: 760 });
    window.dispatchEvent(new Event(LAYOUT_CHANGE_EVENT));
    expect(rects().at(-1)).toEqual({ x: 900, y: 60, width: 480, height: 760 });

    // Nothing moved: nothing sent.
    const sent = posted.length;
    window.dispatchEvent(new Event(LAYOUT_CHANGE_EVENT));
    expect(posted).toHaveLength(sent);
    stop();
    // Stopped: the event goes nowhere.
    box(viewer, { x: 0, y: 0, width: 10, height: 10 });
    window.dispatchEvent(new Event(LAYOUT_CHANGE_EVENT));
    expect(posted.at(-1)).toEqual({ type: "surfaces", update: [], remove: [expect.any(String)] });
  });
});

describe("VideoSlotReporter for one window", () => {
  // The native host streams the Space's desktop unless the slot says which window: a window another Space
  // shows here (`xs:…`) used to be selected in the list while the desktop stayed on screen.
  it("tells the host which window the slot shows, and nothing for a desktop slot", () => {
    const posted: StreamSurfaceMessage[] = [];
    const reporter = new VideoSlotReporter({ postMessage: (m) => posted.push(m) });
    const desktop = document.createElement("div");
    const window1 = document.createElement("div");
    document.body.append(desktop, window1);
    box(desktop, { x: 0, y: 0, width: 400, height: 300 });
    box(window1, { x: 0, y: 320, width: 400, height: 300 });
    const stopDesktop = reporter.register(desktop, { spaceId: "home", tier: "full", interactive: true, radius: 8 }, () => {});
    const stopWindow = reporter.register(window1, { spaceId: "home", tier: "full", interactive: true, radius: 8, windowId: "xs:1a2b" }, () => {});
    const updates = posted.flatMap((m) => (m.type === "surfaces" ? m.update : []));
    expect(updates.find((u) => u.rect.y === 0)).not.toHaveProperty("windowId");
    expect(updates.find((u) => u.rect.y === 320)?.windowId).toBe("xs:1a2b");
    stopDesktop();
    stopWindow();
  });
});

