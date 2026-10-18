// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A window is shown when its page painted, or after a timeout when the
// compositor never lets it paint (Chromium on Wayland, headless GNOME Shell).
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EventEmitter } from "node:events";
import { READY_TIMEOUT_MS, showWhenReady } from "../src/show-when-ready";

class FakeWindow extends EventEmitter {
  destroyed = false;
  isDestroyed() {
    return this.destroyed;
  }
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe("showWhenReady", () => {
  it("shows at ready-to-show, once", () => {
    const win = new FakeWindow();
    const show = vi.fn();
    showWhenReady(win as never, show);
    win.emit("ready-to-show");
    vi.advanceTimersByTime(READY_TIMEOUT_MS * 2);
    expect(show).toHaveBeenCalledTimes(1);
  });

  it("shows after the timeout when ready-to-show never comes (Wayland)", () => {
    const win = new FakeWindow();
    const show = vi.fn();
    showWhenReady(win as never, show);
    vi.advanceTimersByTime(READY_TIMEOUT_MS - 1);
    expect(show).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(show).toHaveBeenCalledTimes(1);
    win.emit("ready-to-show");
    expect(show).toHaveBeenCalledTimes(1);
  });

  it("does not show a window that was closed or destroyed first", () => {
    const closed = new FakeWindow();
    const show = vi.fn();
    showWhenReady(closed as never, show);
    closed.emit("closed");
    vi.advanceTimersByTime(READY_TIMEOUT_MS * 2);
    const gone = new FakeWindow();
    showWhenReady(gone as never, show);
    gone.destroyed = true;
    vi.advanceTimersByTime(READY_TIMEOUT_MS * 2);
    expect(show).not.toHaveBeenCalled();
  });
});
