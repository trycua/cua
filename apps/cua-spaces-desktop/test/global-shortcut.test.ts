// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The global shortcut: registered on macOS only, and when another app holds
// the keys, refused (Settings says so) and tried again on the next read.
import { describe, expect, it, vi } from "vitest";

const register = vi.fn(() => true);
const on = vi.fn();
vi.mock("electron", () => ({ app: { on }, globalShortcut: { register, unregisterAll: vi.fn() } }));
const { GlobalShortcut, installGlobalShortcut } = await import("../src/global-shortcut");

describe("the global shortcut", () => {
  it("is the Mac's only", () => {
    expect(installGlobalShortcut("win32", () => {})).toBeNull();
    expect(installGlobalShortcut("linux", () => {})).toBeNull();
    expect(register).not.toHaveBeenCalled();
    const shortcut = installGlobalShortcut("darwin", () => {});
    expect(register).toHaveBeenCalledWith("Command+Shift+Space", expect.any(Function));
    expect(shortcut?.ensure()).toEqual({ label: "⌘⇧Space", registered: true });
    // Once registered, never again; released when the app quits.
    expect(register).toHaveBeenCalledTimes(1);
    expect(on).toHaveBeenCalledWith("will-quit", expect.any(Function));
  });

  it("says when another app holds the keys, and tries again on the next read", () => {
    const answers = [false, false, true];
    const tries = vi.fn(() => answers.shift() ?? true);
    const pressed = vi.fn();
    let callback = () => {};
    const shortcut = new GlobalShortcut(pressed, (_, cb) => {
      callback = cb;
      return tries();
    });
    expect(shortcut.ensure().registered).toBe(false);
    expect(shortcut.ensure().registered).toBe(false);
    expect(shortcut.ensure().registered).toBe(true);
    expect(shortcut.ensure().registered).toBe(true);
    expect(tries).toHaveBeenCalledTimes(3);
    callback();
    expect(pressed).toHaveBeenCalledOnce();
  });
});
