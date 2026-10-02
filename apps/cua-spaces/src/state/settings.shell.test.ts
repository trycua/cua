// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn(async () => undefined);
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { readMenuBar, readSetting, SETTINGS_KEYS, writeMenuBar } from "./settings";

type ShellWindow = Window & { __CUA_UI_STORAGE__?: Record<string, string> };

// In the app the webview keeps no web storage: settings come from the
// shell's injected values and are written back through ui_storage_set.
describe("settings in the app shell", () => {
  beforeEach(() => {
    window.localStorage.clear();
    invoke.mockClear();
    (window as ShellWindow).__CUA_UI_STORAGE__ = { [SETTINGS_KEYS.menuBar]: "true" };
  });
  afterEach(() => {
    delete (window as ShellWindow).__CUA_UI_STORAGE__;
  });

  it("reads the injected values", () => {
    expect(readMenuBar()).toBe(true);
    expect(readSetting("cua.settings.missing", "x")).toBe("x");
  });

  it("writes through the shell, never to localStorage", async () => {
    writeMenuBar(false);
    expect(readMenuBar()).toBe(false);
    expect(window.localStorage.length).toBe(0);
    await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("ui_storage_set", { key: SETTINGS_KEYS.menuBar, value: "false" }));
  });
});
