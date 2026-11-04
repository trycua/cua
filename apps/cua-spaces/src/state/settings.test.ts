// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { beforeEach, describe, expect, it } from "vitest";

import {
  DEFAULT_HOTKEY,
  DEFAULT_THEME,
  formatHotkey,
  readHotkey,
  readMenuBar,
  readSetting,
  readTheme,
  SETTINGS_KEYS,
  writeHotkey,
  writeMenuBar,
  writeSetting,
  writeTheme,
} from "./settings";

beforeEach(() => {
  window.localStorage.clear();
});

describe("settings persistence", () => {
  it("returns the fallback when a key is missing", () => {
    expect(readSetting("cua.settings.missing", "fallback")).toBe("fallback");
    expect(readHotkey()).toBe(DEFAULT_HOTKEY);
  });

  it("round-trips a written value", () => {
    writeSetting(SETTINGS_KEYS.hotkey, "⌃⌥K");
    expect(readSetting(SETTINGS_KEYS.hotkey, DEFAULT_HOTKEY)).toBe("⌃⌥K");
  });

  it("persists and reads back the hotkey", () => {
    writeHotkey("⌘⇧K");
    expect(readHotkey()).toBe("⌘⇧K");
    expect(window.localStorage.getItem(SETTINGS_KEYS.hotkey)).toBe("⌘⇧K");
  });

  it("defaults the menu-bar flag to false (notch) and round-trips it", () => {
    expect(readMenuBar()).toBe(false);
    writeMenuBar(true);
    expect(readMenuBar()).toBe(true);
    expect(window.localStorage.getItem(SETTINGS_KEYS.menuBar)).toBe("true");
    writeMenuBar(false);
    expect(readMenuBar()).toBe(false);
  });

  it("falls back to notch when the menu-bar store is corrupt", () => {
    window.localStorage.setItem(SETTINGS_KEYS.menuBar, "not json");
    expect(readMenuBar()).toBe(false);
  });

  it("defaults the theme to Island and round-trips Glass", () => {
    expect(DEFAULT_THEME).toBe("island");
    expect(readTheme()).toBe("island");
    writeTheme("glass");
    expect(readTheme()).toBe("glass");
    expect(window.localStorage.getItem(SETTINGS_KEYS.theme)).toBe("glass");
    writeTheme("island");
    expect(readTheme()).toBe("island");
  });

  it("falls back to Island for any unrecognized theme value", () => {
    window.localStorage.setItem(SETTINGS_KEYS.theme, "neon");
    expect(readTheme()).toBe("island");
  });
});

describe("hotkey recorder formatting", () => {
  it("formats the default ⌘⇧Space combo", () => {
    expect(formatHotkey({ key: " ", metaKey: true, shiftKey: true })).toBe("⌘⇧Space");
  });

  it("orders modifiers ⌘⌃⌥⇧ and upper-cases letters", () => {
    expect(
      formatHotkey({ key: "k", metaKey: true, ctrlKey: true, altKey: true, shiftKey: true }),
    ).toBe("⌘⌃⌥⇧K");
  });

  it("labels arrows and escape", () => {
    expect(formatHotkey({ key: "ArrowUp", metaKey: true })).toBe("⌘↑");
    expect(formatHotkey({ key: "Escape", ctrlKey: true })).toBe("⌃Esc");
  });

  it("returns null while only modifiers are held", () => {
    expect(formatHotkey({ key: "Meta", metaKey: true })).toBeNull();
    expect(formatHotkey({ key: "Shift", shiftKey: true })).toBeNull();
  });

  it("requires at least one modifier", () => {
    expect(formatHotkey({ key: "k" })).toBeNull();
    expect(formatHotkey({ key: " " })).toBeNull();
  });
});
