// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { firstNewStream, useAutoOpenFirstStream, type StreamEntry } from "./auto-open-stream";
import { hasStreamChoice } from "./layout-mode";

type P = { windows: StreamEntry[]; ready?: boolean; enabled?: boolean; spaceId?: string; chosen?: boolean };

function setup(initial: P) {
  const onOpen = vi.fn();
  const view = renderHook(
    (p: P) =>
      useAutoOpenFirstStream({
        spaceId: p.spaceId ?? "home",
        enabled: p.enabled ?? true,
        ready: p.ready ?? true,
        windows: p.windows,
        chosen: () => p.chosen ?? false,
        onOpen,
      }),
    { initialProps: initial },
  );
  return { onOpen, rerender: (p: P) => view.rerender(p) };
}

const w = (...ids: string[]) => ids.map((id) => ({ id }));

beforeEach(() => localStorage.clear());

describe("firstNewStream", () => {
  it("is the first listed stream not in the baseline", () => {
    expect(firstNewStream(new Set(["a"]), w("a", "b", "c"))?.id).toBe("b");
    expect(firstNewStream(new Set(["a"]), w("a"))).toBeNull();
  });
});

describe("a home's first new window stream opens by itself", () => {
  it("opens it once when it appears after the page loaded, and never for a later one", () => {
    const { onOpen, rerender } = setup({ windows: [] });
    expect(onOpen).not.toHaveBeenCalled();
    rerender({ windows: w("safari") });
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(onOpen).toHaveBeenCalledWith({ id: "safari" });
    rerender({ windows: w("safari", "notes") });
    rerender({ windows: w("notes") });
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  it("does not open a stream that was already listed when the list loaded", () => {
    const { onOpen, rerender } = setup({ windows: w("old") });
    rerender({ windows: w("old") });
    expect(onOpen).not.toHaveBeenCalled();
  });

  it("waits for the list to load: an empty list before that is not the baseline", () => {
    const { onOpen, rerender } = setup({ windows: [], ready: false });
    rerender({ windows: w("old"), ready: true });
    expect(onOpen).not.toHaveBeenCalled();
    rerender({ windows: w("old", "new"), ready: true });
    expect(onOpen).toHaveBeenCalledWith({ id: "new" });
  });

  it("never overrides a choice the person made for this Space", () => {
    const { onOpen, rerender } = setup({ windows: [], chosen: true });
    rerender({ windows: w("safari"), chosen: true });
    expect(onOpen).not.toHaveBeenCalled();
    // and a later stream does not take its turn either: the first one was the only chance
    rerender({ windows: w("safari", "notes"), chosen: false });
    expect(onOpen).not.toHaveBeenCalled();
  });

  it("does nothing in an ordinary Space", () => {
    const { onOpen, rerender } = setup({ windows: [], enabled: false });
    rerender({ windows: w("safari"), enabled: false });
    expect(onOpen).not.toHaveBeenCalled();
  });

  it("starts over for another Space", () => {
    const { onOpen, rerender } = setup({ windows: [] });
    rerender({ windows: w("a") });
    rerender({ windows: [], spaceId: "other" });
    rerender({ windows: w("b"), spaceId: "other" });
    expect(onOpen).toHaveBeenCalledTimes(2);
  });

  it("a default is not a choice, a saved fold or show is", () => {
    expect(hasStreamChoice("x")).toBe(false);
    localStorage.setItem("cua-spaces:stream-hidden:x", "1");
    expect(hasStreamChoice("x")).toBe(true);
  });
});
