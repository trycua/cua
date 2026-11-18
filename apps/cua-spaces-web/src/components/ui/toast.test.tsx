// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ERROR_TOAST_MS, TOAST_MS, ToastProvider, toast, toastManager } from "./toast";

describe("toast", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    act(() => toastManager.close());
    vi.useRealTimers();
  });

  it("closes info and success toasts by itself, even with the window unfocused", () => {
    render(<ToastProvider>{null}</ToastProvider>);
    act(() => {
      window.dispatchEvent(new Event("blur"));
      toast("Opening qa-gui-linux-local", { description: "Its desktop opens in a separate window." });
      toast("qa-gui-linux-local is ready", { type: "success" });
    });
    expect(screen.getAllByText("Opening qa-gui-linux-local").length).toBeGreaterThan(0);
    act(() => vi.advanceTimersByTime(TOAST_MS + 1000));
    expect(screen.queryByText("Opening qa-gui-linux-local")).toBeNull();
    expect(screen.queryByText("qa-gui-linux-local is ready")).toBeNull();
  });

  it("keeps an error longer, then closes it too (a failed create stayed 10+ min across pages)", () => {
    render(<ToastProvider>{null}</ToastProvider>);
    act(() => {
      window.dispatchEvent(new Event("blur"));
      toast("Could not create the Space: The Cua daemon stopped during this call.", { type: "error" });
    });
    act(() => vi.advanceTimersByTime(TOAST_MS + 1000));
    expect(screen.getAllByText(/^Could not create the Space/).length).toBeGreaterThan(0);
    act(() => vi.advanceTimersByTime(ERROR_TOAST_MS));
    expect(screen.queryByText(/^Could not create the Space/)).toBeNull();
  });

  it("closes a group's failure when the next try in it works", () => {
    render(<ToastProvider>{null}</ToastProvider>);
    act(() => {
      toast("Could not create the Space: The Cua daemon stopped during this call.", { type: "error", group: "create" });
      toast("Couldn't open qa", { type: "error" });
    });
    act(() => vi.advanceTimersByTime(1000));
    act(() => {
      toast("aurora is ready", { type: "success", group: "create" });
    });
    act(() => vi.advanceTimersByTime(1000));
    expect(screen.queryByText(/^Could not create the Space/)).toBeNull();
    expect(screen.getAllByText("aurora is ready").length).toBeGreaterThan(0);
    // Another group's (or no group's) toast stays.
    expect(screen.getAllByText("Couldn't open qa").length).toBeGreaterThan(0);
  });
});
