// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * New Space's runtime fix on Windows: local Spaces run in Docker Desktop,
 * which a clean PC doesn't have. The Electron host says what is missing in
 * `spaces.createOptions` (`local.error`, and `local.fix` for what it can do)
 * and runs the fix in the background; the page reads the options again
 * until the engine answers.
 *
 * | Operation | Electron | SwiftUI | Tauri |
 * |---|---|---|---|
 * | `spaces.fixRuntime {action}` | the host method `spaces.fixRuntime` (`src/bridge/spaces.ts`) | unsupported: a Mac sets up its built-in runtimes itself | unsupported: the Tauri app isn't shipped on Windows |
 */

import { UnsupportedOperationError, type BridgeMode } from "../adapter";
import type { OpCoverage } from "../coverage";
import type { RuntimeFixAction } from "../contracts/new-space";

export interface RuntimeFixOps {
  /** Starts `action` ("install-docker", "start-docker"); it runs on in the host. */
  "spaces.fixRuntime": { args: { action: RuntimeFixAction }; result: null };
}

export const RUNTIME_FIX_OPERATIONS = ["spaces.fixRuntime"] as const satisfies readonly (keyof RuntimeFixOps)[];

export const RUNTIME_FIX_COVERAGE = {
  "spaces.fixRuntime": {
    webkit: { methods: [] },
    electron: { methods: ["spaces.fixRuntime"] },
    tauri: [],
    unsupported: {
      webkit: "a Mac sets up Lume and the built-in Linux runtime itself on the first create",
      tauri: "the Tauri app isn't shipped on Windows (the Electron app is)",
    },
  },
} as const satisfies Record<keyof RuntimeFixOps, OpCoverage>;

/** Hosts with no runtime fix. */
export function unsupportedRuntimeFixOps(mode: BridgeMode) {
  return {
    "spaces.fixRuntime": (() => Promise.reject(new UnsupportedOperationError(mode, "spaces.fixRuntime"))) as () => Promise<null>,
  };
}

/** Arguments that take it down its usual path (coverage tests). */
export const RUNTIME_FIX_ARGS = {
  "spaces.fixRuntime": { action: "start-docker" as RuntimeFixAction },
};
