// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useQuiet } from "./use-quiet";

describe("useQuiet", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("waits until the signature stops changing", () => {
    const { result, rerender } = renderHook(({ sig }) => useQuiet(sig, 100, 1000), { initialProps: { sig: "a" } });
    expect(result.current).toBe(false);
    act(() => void vi.advanceTimersByTime(80));
    rerender({ sig: "b" }); // another read answered: the wait starts over
    act(() => void vi.advanceTimersByTime(80));
    expect(result.current).toBe(false);
    act(() => void vi.advanceTimersByTime(30));
    expect(result.current).toBe(true);
  });

  it("gives up waiting at the cap, however much keeps changing", () => {
    const { result, rerender } = renderHook(({ sig }) => useQuiet(sig, 100, 400), { initialProps: { sig: "0" } });
    for (let i = 1; i <= 6; i++) {
      act(() => void vi.advanceTimersByTime(70));
      rerender({ sig: String(i) });
    }
    expect(result.current).toBe(true);
  });

  it("stays true after later changes, so they update in place", () => {
    const { result, rerender } = renderHook(({ sig }) => useQuiet(sig, 50, 500), { initialProps: { sig: "a" } });
    act(() => void vi.advanceTimersByTime(60));
    expect(result.current).toBe(true);
    rerender({ sig: "b" });
    expect(result.current).toBe(true);
  });
});
