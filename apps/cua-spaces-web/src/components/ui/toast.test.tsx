// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { TOAST_MS, ToastProvider, toast, toastManager } from "./toast";

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

  it("keeps an error until it is dismissed", () => {
    render(<ToastProvider>{null}</ToastProvider>);
    act(() => {
      toast("Couldn't create macOS Space", { type: "error", description: "No room" });
    });
    act(() => vi.advanceTimersByTime(TOAST_MS * 10));
    expect(screen.getAllByText("Couldn't create macOS Space").length).toBeGreaterThan(0);
  });
});
