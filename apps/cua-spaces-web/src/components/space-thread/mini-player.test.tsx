// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { clampWidth, DEFAULT_WIDTH, MiniPlayer, MIN_WIDTH, nearestCorner } from "./mini-player";
import { savedLayout } from "./layout-mode";

class FakeResizeObserver {
  observe() {}
  disconnect() {}
}

const AREA = { w: 900, h: 600 };

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", FakeResizeObserver);
  localStorage.clear();
  HTMLElement.prototype.setPointerCapture = () => {};
  vi.spyOn(HTMLElement.prototype, "clientWidth", "get").mockReturnValue(AREA.w);
  vi.spyOn(HTMLElement.prototype, "clientHeight", "get").mockReturnValue(AREA.h);
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const mount = (props: { onExpand?: () => void; onReserve?: (px: number) => void } = {}) =>
  render(
    <MiniPlayer spaceId="local:a" label="Desktop" aspect={16 / 10} onExpand={props.onExpand ?? (() => {})} onReserve={props.onReserve}>
      <div data-stage="" />
    </MiniPlayer>,
  );

const player = (c: HTMLElement) => c.querySelector<HTMLElement>("[data-mini-player]")!;
const surface = (c: HTMLElement) => player(c).querySelector<HTMLElement>('[role="button"]')!;

describe("thread view's mini-player", () => {
  it("snaps to the nearest corner and keeps its width inside the thread", () => {
    expect(nearestCorner(10, 10, 900, 600)).toBe("tl");
    expect(nearestCorner(890, 10, 900, 600)).toBe("tr");
    expect(nearestCorner(10, 590, 900, 600)).toBe("bl");
    expect(nearestCorner(890, 590, 900, 600)).toBe("br");
    expect(clampWidth(5000, 900, 600, 1.6)).toBeLessThanOrEqual(900 - 24);
    expect(clampWidth(10, 900, 600, 1.6)).toBe(MIN_WIDTH);
    // Never taller than the thread it floats in.
    expect(clampWidth(640, 900, 300, 1.6) / 1.6).toBeLessThanOrEqual(300);
  });

  it("starts in the bottom-right corner, then follows a drag to the corner it is dropped nearest", () => {
    const { container } = mount();
    expect(player(container).dataset.miniPlayer).toBe("br");
    expect(player(container).style.width).toBe(`${DEFAULT_WIDTH}px`);
    const el = surface(container);
    fireEvent.pointerDown(el, { button: 0, pointerId: 1, clientX: 800, clientY: 500 });
    fireEvent.pointerMove(el, { pointerId: 1, clientX: 200, clientY: 120 });
    fireEvent.pointerUp(el, { pointerId: 1, clientX: 200, clientY: 120 });
    expect(player(container).dataset.miniPlayer).toBe("tl");
    // Remembered per Space: a new mount comes back there.
    cleanup();
    expect(player(mount().container).dataset.miniPlayer).toBe("tl");
    expect(JSON.parse(localStorage.getItem("cua-spaces:mini-player:local:a")!)).toMatchObject({ corner: "tl" });
  });

  it("grows from its grip and remembers its size", () => {
    const { container } = mount();
    const grip = container.querySelector<HTMLElement>("[data-mini-player-grip]")!;
    // Bottom-right: the grip is on the bar's left end; dragging it left (toward the middle) grows the player.
    fireEvent.pointerDown(grip, { button: 0, pointerId: 2, clientX: 600, clientY: 590 });
    fireEvent.pointerMove(grip, { pointerId: 2, clientX: 520, clientY: 590 });
    fireEvent.pointerMove(grip, { pointerId: 2, clientX: 500, clientY: 590 });
    fireEvent.pointerUp(grip, { pointerId: 2, clientX: 500, clientY: 590 });
    expect(player(container).style.width).toBe(`${DEFAULT_WIDTH + 100}px`);
    cleanup();
    expect(player(mount().container).style.width).toBe(`${DEFAULT_WIDTH + 100}px`);
  });

  it("goes back to the call view on a click (or its expand arrows), not on a drag", () => {
    const onExpand = vi.fn();
    const { container } = mount({ onExpand });
    const el = surface(container);
    fireEvent.pointerDown(el, { button: 0, pointerId: 1, clientX: 700, clientY: 500 });
    fireEvent.pointerUp(el, { pointerId: 1, clientX: 701, clientY: 500 });
    expect(onExpand).toHaveBeenCalledTimes(1);
    fireEvent.pointerDown(el, { button: 0, pointerId: 1, clientX: 700, clientY: 500 });
    fireEvent.pointerMove(el, { pointerId: 1, clientX: 600, clientY: 400 });
    fireEvent.pointerUp(el, { pointerId: 1, clientX: 600, clientY: 400 });
    expect(onExpand).toHaveBeenCalledTimes(1);
    fireEvent.click(container.querySelector("[data-mini-player-expand]")!);
    expect(onExpand).toHaveBeenCalledTimes(2);
    // Keyboard and accessibility clicks expand too.
    fireEvent.keyDown(el, { key: "Enter" });
    expect(onExpand).toHaveBeenCalledTimes(3);
  });

  it("centres its title on the bar", () => {
    const { container } = mount();
    const label = container.querySelector<HTMLElement>("[data-mini-player-label]")!;
    expect(label.textContent).toBe("Desktop");
    expect(label.className).toContain("text-center");
    expect(label.className).toContain("absolute");
  });

  // Never over the newest message: at the bottom it asks the thread to keep its own height clear.
  it("asks the thread to keep room for it at the bottom, and none at the top", () => {
    const onReserve = vi.fn();
    const { container } = mount({ onReserve });
    const height = parseFloat(player(container).style.height);
    expect(onReserve).toHaveBeenLastCalledWith(height + 12);
    const el = surface(container);
    fireEvent.pointerDown(el, { button: 0, pointerId: 1, clientX: 800, clientY: 500 });
    fireEvent.pointerMove(el, { pointerId: 1, clientX: 800, clientY: 100 });
    fireEvent.pointerUp(el, { pointerId: 1, clientX: 800, clientY: 100 });
    expect(player(container).dataset.miniPlayer).toBe("tr");
    expect(onReserve).toHaveBeenLastCalledWith(0);
  });

  it("remembers the view per Space", () => {
    expect(savedLayout("local:a")).toBe("call");
    localStorage.setItem("cua-spaces:layout:local:a", "thread");
    expect(savedLayout("local:a")).toBe("thread");
    expect(savedLayout("local:b")).toBe("call");
  });
});
