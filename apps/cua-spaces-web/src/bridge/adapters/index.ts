// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { BridgeMode, DataAdapter } from "../adapter";
import { createChildrenStore } from "../children";
import { createNetworkStore } from "../network";
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
export function createAdapter(mode: BridgeMode = detectMode(), win: HostWindow | undefined = globalThis.window as HostWindow | undefined): DataAdapter {
  switch (mode) {
    case "tauri":
      return createTauriAdapter(win);
    case "electron":
      return withChildren(withNetwork(createElectronAdapter(win)));
    case "webkit":
      return withChildren(withNetwork(createWebkitAdapter(win)));
    case "demo":
      return createDemoAdapter(demoOptionsFromSearch(win?.location?.search ?? ""));
  }
}

/** `a` with the Space page's network store over its `network.*` operations
 * (a host that does not answer them leaves the store empty). */
function withNetwork(a: DataAdapter): DataAdapter {
  return a.network ? a : Object.assign(a, { network: createNetworkStore(a) });
}

/** `a` with the child Spaces store over its `spaces.tree`, `children.*`,
 * `budget.*` and `services.*` operations. */
function withChildren(a: DataAdapter): DataAdapter {
  return a.children ? a : Object.assign(a, { children: createChildrenStore(a) });
}
