// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { CallSplit, DEFAULT_SPLIT, DIVIDER_HIT_PX } from "./call-split";

class FakeResizeObserver {
  observe() {}
  disconnect() {}
}

beforeEach(() => {
  vi.stubGlobal("ResizeObserver", FakeResizeObserver);
  localStorage.clear();
  HTMLElement.prototype.setPointerCapture = () => {};
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
});

describe("<CallSplit>", () => {
  // The owner asked for an easier grab: a taller hit area, the same small bar.
  it("takes the drag on a row taller than its bar, with the row-resize cursor", () => {
    const { container } = render(<CallSplit stage={<div />} below={<div />} />);
    const divider = container.querySelector<HTMLElement>("[data-call-divider]")!;
    expect(DIVIDER_HIT_PX).toBeGreaterThanOrEqual(16);
    expect(divider.className).toContain(`h-${DIVIDER_HIT_PX / 4}`);
    expect(divider.className).toContain("cursor-row-resize");
    const bar = divider.querySelector("[data-call-divider-bar]")!;
    // The bar stays quiet: 4 px tall, a hover and a drag state.
    expect(bar.className).toContain("h-1");
    expect(bar.className).toContain("group-hover:");
  });

  it("follows a drag and remembers the split", () => {
    const { container } = render(<CallSplit stage={<div />} below={<div />} />);
    const root = container.querySelector<HTMLElement>("[data-call-split]")!;
    root.getBoundingClientRect = () => ({ x: 0, y: 0, top: 0, left: 0, width: 800, height: 1000, right: 800, bottom: 1000, toJSON: () => ({}) }) as DOMRect;
    const divider = container.querySelector<HTMLElement>("[data-call-divider]")!;
    expect(root.dataset.callSplit).toBe(DEFAULT_SPLIT.toFixed(2));
    fireEvent.pointerDown(divider, { button: 0, pointerId: 1, clientY: 520 });
    const bar = divider.querySelector("[data-call-divider-bar]")!;
    expect(bar.className).toContain("bg-ring");
    fireEvent.pointerMove(divider, { pointerId: 1, clientY: 700 });
    fireEvent.pointerUp(divider, { pointerId: 1, clientY: 700 });
    expect(root.dataset.callSplit).toBe("0.70");
    expect(localStorage.getItem("cua-spaces:call-split")).toBe("0.7");
    cleanup();
    const again = render(<CallSplit stage={<div />} below={<div />} />);
    expect(again.container.querySelector<HTMLElement>("[data-call-split]")!.dataset.callSplit).toBe("0.70");
  });
});
