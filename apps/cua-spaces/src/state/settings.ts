// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { core } from "../core";

/**
 * User settings for the Cua Spaces switcher, plus a tiny typed storage
 * wrapper (the shell's settings file in the app, localStorage elsewhere) that tolerates missing or blocked storage (private windows, disabled
 * site data, non-browser test runners). Every read falls back to a default and
 * every write is best-effort, so a throwing or absent `localStorage` never
 * breaks the panel.
 */

/** localStorage keys. Namespaced so they never collide with other app state. */
export const SETTINGS_KEYS = {
  hotkey: "cua.settings.hotkey",
  menuBar: "cua.settings.menuBar",
  theme: "cua.settings.theme",
  experiments: "cua.settings.experiments",
} as const;

/** Default global hotkey that raises the switcher (display + persisted value). */
export const DEFAULT_HOTKEY = "⌘⇧Space"; // ⌘⇧Space

type KeyValueStore = Pick<Storage, "getItem" | "setItem">;

/**
 * In the app, the shell's storage: values injected before the page runs
 * (`window.__CUA_UI_STORAGE__`, from the app's data directory) and written
 * back through `ui_storage_set`, which updates every open window. The
 * webview itself keeps no web storage (src-tauri/src/webview_data.rs).
 */
function shellStorage(): KeyValueStore | null {
  const values = (window as { __CUA_UI_STORAGE__?: Record<string, string> }).__CUA_UI_STORAGE__;
  if (!values) return null;
  return {
    getItem: (key) => (Object.prototype.hasOwnProperty.call(values, key) ? (values[key] ?? null) : null),
    setItem: (key, value) => {
      values[key] = String(value);
      void import("@tauri-apps/api/core")
        .then(({ invoke }) => invoke("ui_storage_set", { key, value: String(value) }))
        .catch(() => {
          // Ignore: settings are a convenience, not a source of truth.
        });
    },
  };
}

/** Best-effort handle to the settings storage; null when unavailable. The
 * shell's storage in the app, `localStorage` in a plain browser or test. */
function storage(): KeyValueStore | null {
  try {
    if (typeof window === "undefined") return null;
    return shellStorage() ?? window.localStorage;
  } catch {
    // Some browsers throw on property access when site data is blocked.
    return null;
  }
}

/** Read a string setting, falling back when missing or storage is unavailable. */
export function readSetting(key: string, fallback: string): string {
  try {
    const value = storage()?.getItem(key);
    return value ?? fallback;
  } catch {
    return fallback;
  }
}

/** Persist a string setting; a no-op when storage is unavailable or throws. */
export function writeSetting(key: string, value: string): void {
  try {
    storage()?.setItem(key, value);
  } catch {
    // Ignore: storage is a convenience, not a source of truth.
  }
}

/** Read + JSON-parse a setting, falling back on any error. */
export function readJson<T>(key: string, fallback: T): T {
  const raw = readSetting(key, "");
  if (!raw) return fallback;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return fallback;
  }
}

/** JSON-stringify + persist a setting; a no-op on failure. */
export function writeJson<T>(key: string, value: T): void {
  try {
    writeSetting(key, JSON.stringify(value));
  } catch {
    // Ignore serialization/storage failures.
  }
}

/* ---------------------------------------------------------------------------
 * Presentation: notch vs menu-bar entry point
 * ------------------------------------------------------------------------- */

/**
 * Whether the switcher entry point is the macOS menu-bar status item rather
 * than the ambient notch tab. Distinct from `DisplayStyle` (the notch/no-notch
 * layout, detected per-monitor): in menu-bar mode nothing shows in the notch
 * (no tab, no hover cue, no window-drag teleport box), the same rule as the
 * core's hidden notch state and the SwiftUI app.
 * Defaults to `false` (notch).
 */
export function readMenuBar(): boolean {
  return readJson<boolean>(SETTINGS_KEYS.menuBar, false);
}

/** Persist the menu-bar presentation preference. */
export function writeMenuBar(enabled: boolean): void {
  writeJson(SETTINGS_KEYS.menuBar, enabled);
}

/* ---------------------------------------------------------------------------
 * Switcher theme: Island (default) vs Glass
 * ------------------------------------------------------------------------- */

/**
 * The switcher panel's visual theme. `island` (default) is an opaque black
 * panel flush to the top of the screen with concave top corners that expands
 * out of the notch like macOS's Dynamic Island; `glass` is the translucent
 * acrylic panel that floats below the notch.
 */
export type SwitcherTheme = "island" | "glass";

/** The default theme applied when nothing is persisted. */
export const DEFAULT_THEME: SwitcherTheme = "island";

/** Read the persisted switcher theme, defaulting to Island. */
export function readTheme(): SwitcherTheme {
  return readSetting(SETTINGS_KEYS.theme, DEFAULT_THEME) === "glass" ? "glass" : "island";
}

/** Persist the chosen switcher theme. */
export function writeTheme(theme: SwitcherTheme): void {
  writeSetting(SETTINGS_KEYS.theme, theme);
}

/* ---------------------------------------------------------------------------
 * Global hotkey recorder
 * ------------------------------------------------------------------------- */

/** The subset of a keyboard event the recorder needs (easy to unit test). */
export interface KeyCombo {
  key: string;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
}

/**
 * Format a pressed combo into a compact glyph string like `⌘⇧Space`. Returns
 * null while only modifiers are held (still "recording"), or when no modifier
 * accompanies the key — a global hotkey must include at least one modifier.
 * Modifier order matches Apple's menu convention shown against ⌘ first here so
 * the default renders as `⌘⇧Space`.
 */
export function formatHotkey(combo: KeyCombo): string | null {
  return core("settings.formatHotkey", {
    combo: {
      key: combo.key,
      metaKey: Boolean(combo.metaKey),
      ctrlKey: Boolean(combo.ctrlKey),
      altKey: Boolean(combo.altKey),
      shiftKey: Boolean(combo.shiftKey),
    },
  });
}

/** Read the persisted hotkey, defaulting to `⌘⇧Space`. */
export function readHotkey(): string {
  return readSetting(SETTINGS_KEYS.hotkey, DEFAULT_HOTKEY);
}

/** Persist the chosen hotkey preference. */
export function writeHotkey(value: string): void {
  writeSetting(SETTINGS_KEYS.hotkey, value);
}

