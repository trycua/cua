// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { createWebkitAdapter } from "../adapters/webkit";
import type { HostWindow } from "../detect";
import { dragRegions, subtract, syncDragRegions, type Rect } from "../drag-regions";
import type { WebkitRequest } from "../webkit-protocol";

const r = (x: number, y: number, width: number, height: number): Rect => ({ x, y, width, height });

describe("drag regions", () => {
  it("cuts a hole into a box", () => {
    expect(subtract(r(0, 0, 100, 40), r(200, 0, 10, 10))).toEqual([r(0, 0, 100, 40)]);
    // A button in the middle of the top bar: left and right of it, and above and below.
    expect(subtract(r(0, 0, 100, 40), r(40, 8, 20, 24))).toEqual([r(0, 0, 100, 8), r(0, 32, 100, 8), r(0, 8, 40, 24), r(60, 8, 40, 24)]);
    expect(subtract(r(0, 0, 100, 40), r(0, 0, 100, 40))).toEqual([]);
  });

  it("leaves the top bar's buttons clickable", () => {
    // The top bar beside the traffic lights, with the sidebar toggle and the search field.
    const bar = r(78, 0, 1200, 38);
    const regions = dragRegions([bar], [r(90, 7, 24, 24), r(900, 5, 240, 28), r(0, 0, 0, 0)]);
    const inside = (x: number, y: number) => regions.some((g) => x >= g.x && x < g.x + g.width && y >= g.y && y < g.y + g.height);
    expect(inside(100, 15)).toBe(false);
    expect(inside(1000, 15)).toBe(false);
    expect(inside(500, 15)).toBe(true);
    expect(inside(95, 2)).toBe(true);
  });

  it("measures the page's .app-drag boxes and sends them when they change", async () => {
    document.body.innerHTML = `<header class="app-drag"><button>Toggle</button></header><div class="app-no-drag"></div>`;
    const boxes = new Map<Element, Rect>([
      [document.querySelector("header")!, r(0, 0, 800, 38)],
      [document.querySelector("button")!, r(10, 5, 28, 28)],
      [document.querySelector(".app-no-drag")!, r(700, 0, 0, 0)],
    ]);
    for (const [el, b] of boxes) {
      el.getBoundingClientRect = () => ({ left: b.x, top: b.y, width: b.width, height: b.height }) as DOMRect;
    }
    const sent: Rect[][] = [];
    const stop = syncDragRegions(window, (rects) => sent.push(rects));
    await new Promise((resolve) => requestAnimationFrame(() => resolve(null)));
    expect(sent).toHaveLength(1);
    expect(sent[0]).toEqual(dragRegions([r(0, 0, 800, 38)], [r(10, 5, 28, 28)]));
    // A change that moves nothing sends nothing.
    document.body.setAttribute("class", "x");
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(sent).toHaveLength(1);
    stop();
    document.body.innerHTML = "";
  });

  it("the webkit adapter sends them to the window in the page only", async () => {
    const sent: string[] = [];
    const handler = {
      postMessage: (m: unknown) => {
        const req = m as WebkitRequest;
        sent.push(req.method);
        return Promise.resolve({ id: req.id, ok: true, result: null });
      },
    };
    // A stand-in window (the other tests'): no DOM, no drag regions.
    const bare = createWebkitAdapter({ webkit: { messageHandlers: { cua: handler } } } as HostWindow);
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(sent).toEqual([]);
    bare.dispose?.();
    // The page.
    document.body.innerHTML = `<header class="app-drag"></header>`;
    document.querySelector("header")!.getBoundingClientRect = () => ({ left: 0, top: 0, width: 800, height: 38 }) as DOMRect;
    const w = window as unknown as HostWindow;
    w.webkit = { messageHandlers: { cua: handler } };
    const adapter = createWebkitAdapter(w);
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(sent).toContain("window.setDragRegions");
    adapter.dispose?.();
    delete (w as { webkit?: unknown }).webkit;
    document.body.innerHTML = "";
  });
});
