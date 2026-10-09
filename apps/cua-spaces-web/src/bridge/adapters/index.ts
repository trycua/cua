// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { BridgeMode, DataAdapter } from "../adapter";
import { detectMode, type HostWindow } from "../detect";
import { createDemoAdapter, demoOptionsFromSearch } from "./demo";
import { createElectronAdapter } from "./electron";
import { createTauriAdapter } from "./tauri";
import { createWebkitAdapter } from "./webkit";

export { createDemoAdapter, demoOptionsFromSearch, type DemoOptions } from "./demo";
export { createElectronAdapter } from "./electron";
export { createTauriAdapter, TAURI_EVENTS } from "./tauri";
export { createWebkitAdapter } from "./webkit";

/** The adapter for `mode` (default: detected from the window). */
export function createAdapter(
  mode: BridgeMode = detectMode(),
  win: HostWindow | undefined = globalThis.window as HostWindow | undefined,
): DataAdapter {
  switch (mode) {
    case "tauri":
      return createTauriAdapter(win);
    case "electron":
      return createElectronAdapter(win);
    case "webkit":
      return createWebkitAdapter(win);
    case "demo":
      return createDemoAdapter(demoOptionsFromSearch(win?.location?.search ?? ""));
  }
}
