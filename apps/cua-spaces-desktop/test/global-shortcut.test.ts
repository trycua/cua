// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The global shortcut: registered on macOS only, and when another app holds
// the keys, refused (Settings says so) and tried again on the next read.
import { describe, expect, it, vi } from "vitest";

const register = vi.fn(() => true);
const on = vi.fn();
vi.mock("electron", () => ({ app: { on }, globalShortcut: { register, unregisterAll: vi.fn() } }));
const { GlobalShortcut, installGlobalShortcut, isAccelerator, shortcutLabel } = await import("../src/global-shortcut");

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

describe("changing the global shortcut (Settings, General)", () => {
  const make = (answers: Record<string, boolean>) => {
    const registered: string[] = [];
    const unregistered: string[] = [];
    const saved: string[] = [];
    const shortcut = new GlobalShortcut(
      () => {},
      (a) => {
        if (answers[a] === false) return false;
        registered.push(a);
        return true;
      },
      "Command+Shift+Space",
      (a) => unregistered.push(a),
      (a) => saved.push(a),
    );
    return { shortcut, registered, unregistered, saved };
  };

  it("moves to the new keys, lets go of the old ones and remembers the choice", () => {
    const { shortcut, registered, unregistered, saved } = make({});
    shortcut.ensure();
    expect(shortcut.change("Control+Alt+K")).toEqual({ label: "\u2303\u2325K", registered: true });
    expect(registered).toEqual(["Command+Shift+Space", "Control+Alt+K"]);
    expect(unregistered).toEqual(["Command+Shift+Space"]);
    expect(saved).toEqual(["Control+Alt+K"]);
    expect(shortcut.ensure().label).toBe("\u2303\u2325K");
  });

  it("keeps the old keys and says why when another app holds the new ones", () => {
    const { shortcut, unregistered, saved } = make({ "Command+K": false });
    shortcut.ensure();
    expect(() => shortcut.change("Command+K")).toThrow("Another app is using \u2318K");
    expect(unregistered).toEqual([]);
    expect(saved).toEqual([]);
    expect(shortcut.ensure()).toEqual({ label: "\u2318\u21e7Space", registered: true });
  });

  it("takes only shortcuts with Command, Control or Option and one key", () => {
    for (const ok of ["Command+Shift+Space", "Control+Alt+K", "Alt+1", "Command+F12", "Shift+Command+Up"]) expect(isAccelerator(ok), ok).toBe(true);
    for (const bad of ["K", "Shift+K", "Command", "Command+Shift", "Command+kk", "Command+F25", "", "Command+Shift+Space+K"]) expect(isAccelerator(bad), bad).toBe(false);
    expect(() => make({}).shortcut.change("Shift+K")).toThrow("Not a shortcut");
    expect(shortcutLabel("Command+Shift+Space")).toBe("\u2318\u21e7Space");
  });

  it("starts on the keys chosen last time, and on the default when the saved ones are not a shortcut", () => {
    register.mockClear();
    installGlobalShortcut("darwin", () => {}, { read: () => "Control+Alt+K", write: () => {} });
    expect(register).toHaveBeenLastCalledWith("Control+Alt+K", expect.any(Function));
    installGlobalShortcut("darwin", () => {}, { read: () => "nonsense", write: () => {} });
    expect(register).toHaveBeenLastCalledWith("Command+Shift+Space", expect.any(Function));
  });
});
