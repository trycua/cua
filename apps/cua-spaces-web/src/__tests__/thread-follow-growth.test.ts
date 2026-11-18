// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A note drawn under the last message (one only you see, a "woke up" line)
// grows the thread without a new item: the thread stays at its end when you
// were there, and leaves you where you were when you had scrolled up.

import { afterEach, describe, expect, it, vi } from "vitest";

import { followGrowth } from "@/components/space-thread/thread-view";

let grow: (() => void) | null = null;
class FakeResizeObserver {
  constructor(cb: () => void) {
    grow = cb;
  }
  observe() {}
  disconnect() {
    grow = null;
  }
}

afterEach(() => vi.unstubAllGlobals());

function scroller(height: number) {
  const el = document.createElement("div");
  let h = height;
  Object.defineProperty(el, "scrollHeight", { get: () => h });
  return { el, setHeight: (n: number) => (h = n) };
}

describe("the thread follows a note added under the last message", () => {
  it("stays at the end when you were there", () => {
    vi.stubGlobal("ResizeObserver", FakeResizeObserver);
    const { el, setHeight } = scroller(500);
    el.scrollTop = 500;
    const stop = followGrowth(el, document.createElement("div"), () => true);
    setHeight(640);
    grow!();
    expect(el.scrollTop).toBe(640);
    stop();
    expect(grow).toBeNull();
  });

  it("leaves you where you were when you had scrolled up", () => {
    vi.stubGlobal("ResizeObserver", FakeResizeObserver);
    const { el, setHeight } = scroller(500);
    el.scrollTop = 120;
    followGrowth(el, document.createElement("div"), () => false);
    setHeight(640);
    grow!();
    expect(el.scrollTop).toBe(120);
  });
});
