// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { useInView } from "./use-in-view";

type Callback = (entries: { isIntersecting: boolean }[]) => void;

/** An IntersectionObserver the test drives. */
function fakeObserver() {
  const observers: { cb: Callback; margin?: string; observed: number; disconnected: boolean }[] = [];
  class FakeIO {
    o: (typeof observers)[number];
    constructor(cb: Callback, init?: { rootMargin?: string }) {
      this.o = { cb, margin: init?.rootMargin, observed: 0, disconnected: false };
      observers.push(this.o);
    }
    observe() {
      this.o.observed += 1;
    }
    disconnect() {
      this.o.disconnected = true;
    }
  }
  vi.stubGlobal("IntersectionObserver", FakeIO);
  return observers;
}

describe("useInView", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("is false until the element nears the viewport, and follows it out again", () => {
    const observers = fakeObserver();
    const el = document.createElement("div");
    const { result, unmount } = renderHook(() => useInView({ current: el }));
    // A grid of many Spaces: an off-screen tile asks for nothing.
    expect(result.current).toBe(false);
    expect(observers).toHaveLength(1);
    expect(observers[0]!.margin).toBe("400px");
    act(() => observers[0]!.cb([{ isIntersecting: true }]));
    expect(result.current).toBe(true);
    act(() => observers[0]!.cb([{ isIntersecting: false }]));
    expect(result.current).toBe(false);
    unmount();
    expect(observers[0]!.disconnected).toBe(true);
  });

  it("is true where the page cannot tell", () => {
    vi.stubGlobal("IntersectionObserver", undefined);
    const { result } = renderHook(() => useInView({ current: document.createElement("div") }));
    expect(result.current).toBe(true);
  });
});
