// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { createWebkitAdapter } from "../adapters/webkit";
import type { HostWindow } from "../detect";
import type { Rect } from "../drag-regions";
import type { WebkitRequest } from "../webkit-protocol";
import { clip, measureDropTargets, setWindowDropTarget, syncDropTargets, useWindowDropTarget, windowDropTargetOf, type DropTargetRect } from "../window-drop";

const r = (x: number, y: number, width: number, height: number): Rect => ({ x, y, width, height });
const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve(null)));

function place(el: Element, b: Rect) {
  el.getBoundingClientRect = () => ({ left: b.x, top: b.y, width: b.width, height: b.height }) as DOMRect;
}

afterEach(() => {
  document.body.innerHTML = "";
  setWindowDropTarget(null);
});

describe("window drop targets", () => {
  it("clips a box", () => {
    expect(clip(r(0, 0, 100, 100), r(50, 50, 100, 100))).toEqual(r(50, 50, 50, 50));
    expect(clip(r(0, 0, 10, 10), r(20, 20, 5, 5))).toBeNull();
  });

  it("measures each tile's visible box with its Space id", () => {
    document.body.innerHTML = `
      <main style="overflow: auto">
        <div data-window-drop="local:a"></div>
        <div data-window-drop="local:b"></div>
        <div data-window-drop="local:c"></div>
      </main>`;
    const [a, b, c] = [...document.querySelectorAll("[data-window-drop]")];
    // The scrolling page below the 38 px top bar; b is half scrolled out, c entirely.
    place(document.querySelector("main")!, r(0, 38, 1000, 600));
    place(a!, r(20, 60, 232, 180));
    place(b!, r(20, 560, 232, 180));
    place(c!, r(20, 900, 232, 180));
    expect(measureDropTargets(window)).toEqual<DropTargetRect[]>([
      { spaceId: "local:a", ...r(20, 60, 232, 180) },
      { spaceId: "local:b", ...r(20, 560, 232, 78) },
    ]);
  });

  it("offers none while a dialog covers the page", () => {
    document.body.innerHTML = `<div data-window-drop="local:a"></div><div role="dialog"></div>`;
    place(document.querySelector("[data-window-drop]")!, r(0, 0, 100, 100));
    expect(measureDropTargets(window)).toEqual([]);
  });

  it("sends them when they change, and only then", async () => {
    document.body.innerHTML = `<div data-window-drop="local:a"></div>`;
    place(document.querySelector("[data-window-drop]")!, r(10, 10, 100, 100));
    const sent: DropTargetRect[][] = [];
    const stop = syncDropTargets(window, (t) => sent.push(t));
    await frame();
    expect(sent).toEqual([[{ spaceId: "local:a", ...r(10, 10, 100, 100) }]]);
    document.body.setAttribute("class", "x");
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(sent).toHaveLength(1);
    // The tiles left (another page): an empty list goes out.
    document.body.innerHTML = "";
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(sent.at(-1)).toEqual([]);
    stop();
  });

  it("highlights the tile the host names", () => {
    const { result } = renderHook(() => useWindowDropTarget());
    expect(result.current).toBeNull();
    act(() => setWindowDropTarget(windowDropTargetOf({ spaceId: "local:a" })));
    expect(result.current).toBe("local:a");
    act(() => setWindowDropTarget(windowDropTargetOf({ spaceId: null })));
    expect(result.current).toBeNull();
    expect(windowDropTargetOf(null)).toBeNull();
    expect(windowDropTargetOf({ spaceId: 4 })).toBeNull();
  });

  it("the webkit adapter sends the tiles and follows the host's highlight", async () => {
    const sent: WebkitRequest[] = [];
    const handler = {
      postMessage: (m: unknown) => {
        const req = m as WebkitRequest;
        sent.push(req);
        return Promise.resolve({ id: req.id, ok: true, result: null });
      },
    };
    document.body.innerHTML = `<div data-window-drop="local:a"></div>`;
    place(document.querySelector("[data-window-drop]")!, r(0, 50, 232, 180));
    const w = window as unknown as HostWindow;
    w.webkit = { messageHandlers: { cua: handler } };
    const adapter = createWebkitAdapter(w);
    await new Promise((resolve) => setTimeout(resolve, 50));
    const req = sent.find((s) => s.method === "window.setDropTargets");
    expect(req?.args).toEqual({ targets: [{ spaceId: "local:a", ...r(0, 50, 232, 180) }] });
    const { result } = renderHook(() => useWindowDropTarget());
    act(() => {
      window.dispatchEvent(new CustomEvent("cua:event", { detail: { event: "window.dropTarget", payload: { spaceId: "local:a" } } }));
    });
    expect(result.current).toBe("local:a");
    // Closing the bridge drops the highlight.
    act(() => adapter.dispose?.());
    expect(result.current).toBeNull();
    delete (w as { webkit?: unknown }).webkit;
  });
});
