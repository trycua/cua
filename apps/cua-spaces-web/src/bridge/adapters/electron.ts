// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Electron shell: a native host like the SwiftUI app. Its main process
 * answers the SwiftUI host's methods (`../webkit-protocol.ts`) from the same
 * Rust library, so this is the webkit adapter over the preload's
 * `window.cuaDesktop` (`../transport.ts`), plus what only Electron answers
 * (`ELECTRON_HOST_METHODS`): a Space's media ticket for the page's video
 * (`spaces.openStream`), its first run and the relay allowance.
 */

import type { DataAdapter } from "../adapter";
import type { HostWindow } from "../detect";
import { createWebkitAdapter, type WebkitAdapterOptions } from "./webkit";

export function createElectronAdapter(
  win: HostWindow = globalThis.window as HostWindow,
  options: Omit<WebkitAdapterOptions, "host"> = {},
): DataAdapter {
  return createWebkitAdapter(win, { ...options, host: "electron" });
}
