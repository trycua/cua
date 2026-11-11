// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";

import { hasStreamChoice, savedStreamHidden, useStreamHidden } from "./layout-mode";

beforeEach(() => localStorage.clear());

describe("savedStreamHidden", () => {
  it("shows the stream of an ordinary Space and folds a home's until the person chooses", () => {
    expect(savedStreamHidden("a")).toBe(false);
    expect(savedStreamHidden("a", true)).toBe(true);
  });

  it("keeps the person's choice over the default, either way, per Space", () => {
    localStorage.setItem("cua-spaces:stream-hidden:home", "0");
    localStorage.setItem("cua-spaces:stream-hidden:other", "1");
    expect(savedStreamHidden("home", true)).toBe(false);
    expect(savedStreamHidden("other", false)).toBe(true);
    expect(savedStreamHidden("third", true)).toBe(true);
  });
});

describe("useStreamHidden", () => {
  it("shows a home's folded stream for this visit without making that a choice", () => {
    const { result, unmount } = renderHook(() => useStreamHidden("home", true));
    expect(result.current[0]).toBe(true);
    act(() => result.current[2]());
    expect(result.current[0]).toBe(false);
    // Nothing was saved: the next visit starts folded again, and an app opened later still opens by itself.
    expect(hasStreamChoice("home")).toBe(false);
    unmount();
    expect(renderHook(() => useStreamHidden("home", true)).result.current[0]).toBe(true);
  });

  it("keeps what the person chose", () => {
    const { result } = renderHook(() => useStreamHidden("home", true));
    act(() => result.current[1](false));
    expect(hasStreamChoice("home")).toBe(true);
    expect(renderHook(() => useStreamHidden("home", true)).result.current[0]).toBe(false);
  });
});
