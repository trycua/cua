// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { nativeTheme } from "electron";
import type { ResolvedTheme, ThemeSource, ThemeState } from "./channels";
import { readSettings, writeSettings } from "./settings";

// The web UI's `--background` (apps/cua-spaces-web/src/index.css), which is
// also the top bar behind the Windows/Linux title bar overlay, in sRGB. Used
// for the window background so there is no flash before the page paints.
// Window control glyphs on the overlay: the web UI's `--foreground`.
export { BACKGROUND, SYMBOL } from "./overlay";

export const resolvedTheme = (): ResolvedTheme =>
  nativeTheme.shouldUseDarkColors ? "dark" : "light";

export const themeState = (): ThemeState => ({
  source: nativeTheme.themeSource,
  resolved: resolvedTheme(),
});

export function initTheme(): void {
  nativeTheme.themeSource = readSettings().themeSource ?? "system";
}

export function setThemeSource(source: ThemeSource): ThemeState {
  nativeTheme.themeSource = source;
  writeSettings({ themeSource: source });
  return themeState();
}
