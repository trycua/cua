// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The system's preferred languages, most preferred first, from the host:
 *
 * - SwiftUI (macOS): `window.cuaHost.languages`, `Locale.preferredLanguages`
 *   (apps/cua-spaces-macos, `WebUIWindowController.hostScript`).
 * - Electron (macOS, Windows, Linux): `window.cuaDesktop.languages`,
 *   `app.getPreferredSystemLanguages()` (apps/cua-spaces-desktop, preload).
 * - Browser, demo and Tauri: `navigator.languages`.
 *
 * The hosts' lists come first because a desktop web view's
 * `navigator.language` is the engine's UI language, not the person's list.
 */

export interface LanguageHost {
  cuaHost?: { languages?: readonly string[] };
  cuaDesktop?: { languages?: readonly string[] };
  navigator?: { languages?: readonly string[]; language?: string };
}

const clean = (list: readonly unknown[] | undefined): string[] => (list ?? []).filter((x): x is string => typeof x === "string" && x.trim() !== "");

export function hostLanguages(win: LanguageHost | undefined = globalThis as LanguageHost): string[] {
  const native = clean(win?.cuaHost?.languages);
  if (native.length > 0) return native;
  const desktop = clean(win?.cuaDesktop?.languages);
  if (desktop.length > 0) return desktop;
  const nav = win?.navigator;
  const browser = clean(nav?.languages);
  if (browser.length > 0) return browser;
  return clean(nav?.language ? [nav.language] : []);
}
