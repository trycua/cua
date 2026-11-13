// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { DEFAULT_KEYBINDINGS, matchesShortcut, resolveCommand, shortcutLabel } from "./keybindings";

const key = (k: string, mods: Partial<Record<"metaKey" | "ctrlKey" | "shiftKey" | "altKey", boolean>> = {}) => ({
  key: k,
  metaKey: false,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  ...mods,
});

describe("keybindings", () => {
  it("binds every default to a unique key", () => {
    const keys = DEFAULT_KEYBINDINGS.map((b) => b.key);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("treats mod as Cmd on macOS and Ctrl elsewhere", () => {
    expect(matchesShortcut(key("k", { metaKey: true }), "mod+k", true)).toBe(true);
    expect(matchesShortcut(key("k", { ctrlKey: true }), "mod+k", true)).toBe(false);
    expect(matchesShortcut(key("k", { ctrlKey: true }), "mod+k", false)).toBe(true);
  });

  it("requires the exact modifiers", () => {
    expect(matchesShortcut(key("L", { metaKey: true, shiftKey: true }), "mod+shift+l", true)).toBe(true);
    expect(matchesShortcut(key("l", { metaKey: true }), "mod+shift+l", true)).toBe(false);
  });

  it("resolves a key event to its command", () => {
    expect(resolveCommand(key("k", { metaKey: true }), DEFAULT_KEYBINDINGS, true)).toBe("commandPalette.toggle");
    expect(resolveCommand(key(",", { metaKey: true }), DEFAULT_KEYBINDINGS, true)).toBe("nav.settings");
    expect(resolveCommand(key("k"), DEFAULT_KEYBINDINGS, true)).toBeNull();
  });

  it("labels shortcuts for the platform", () => {
    expect(shortcutLabel("mod+shift+l", true)).toEqual(["⌘", "⇧", "L"]);
    expect(shortcutLabel("mod+k", false)).toEqual(["Ctrl", "K"]);
  });
});
