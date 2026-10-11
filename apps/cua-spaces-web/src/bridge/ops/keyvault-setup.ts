// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Keyvault page's "Set up Keyvault": creates the vault the way the
 * core's setup form offers (`KeyvaultPage.form`, mode `setup`). Touch ID
 * keeps the vault key in the login keychain; a passphrase is the other way.
 * The answer is the recovery key, shown once.
 *
 * | Operation | Tauri | SwiftUI |
 * |---|---|---|
 * | `keyvault.setup {passphrase?}` | `keyvault_setup`, `keyvault_setup_passphrase` | `keyvault.setup` → `KeyvaultModel.submitCredential` |
 *
 * A passphrase never crosses the SwiftUI bridge (`native_only`: the host
 * opens its own form), as for `keyvault.unlock`; the Electron shell answers
 * the same method.
 */

import type { OpCoverage } from "../coverage";
import type { OpArgs, OpResult } from "../protocol";

/** What setup answers. */
export interface KeyvaultSetupResult {
  /** The recovery key, shown once (null when the host shows it itself). */
  recoveryKey: string | null;
}

declare module "../protocol" {
  interface HostOperations {
    /** Creates the Keyvault (Touch ID, or this passphrase). */
    "keyvault.setup": { args: { passphrase?: string | null }; result: KeyvaultSetupResult };
  }
}

export const KEYVAULT_SETUP_OPERATIONS = ["keyvault.setup"] as const;
export type KeyvaultSetupOp = (typeof KEYVAULT_SETUP_OPERATIONS)[number];

export const KEYVAULT_SETUP_WEBKIT_METHODS = KEYVAULT_SETUP_OPERATIONS;

export const KEYVAULT_SETUP_COVERAGE = {
  "keyvault.setup": {
    webkit: { methods: ["keyvault.setup"], byDesign: "a passphrase never crosses the bridge: native_only" },
    tauri: ["keyvault_setup", "keyvault_setup_passphrase"],
  },
} as const satisfies Record<KeyvaultSetupOp, OpCoverage>;

export const KEYVAULT_SETUP_ARGS: { [K in KeyvaultSetupOp]: OpArgs<K> } = {
  "keyvault.setup": { passphrase: null },
};

/** The command a person can run instead (any host). */
export const KEYVAULT_SETUP_COMMAND = "cua keyvault init";

/** A host's answer as the page reads it: a key or nothing. */
export function setupResultFromWire(raw: unknown): KeyvaultSetupResult {
  const key = typeof raw === "string" ? raw : (raw as { recoveryKey?: unknown } | null)?.recoveryKey;
  return { recoveryKey: typeof key === "string" && key ? key : null };
}

type Handlers = { [K in KeyvaultSetupOp]: (args: OpArgs<K>) => Promise<OpResult<K>> };
type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
type Request = <T>(method: "keyvault.setup", args?: Record<string, unknown>) => Promise<T>;

/** Tauri: `keyvault_setup` answers the recovery key (or null). */
export function tauriKeyvaultSetupOps(invoke: Invoke): Handlers {
  return {
    "keyvault.setup": async ({ passphrase }) =>
      setupResultFromWire(
        passphrase ? await invoke<string | null>("keyvault_setup_passphrase", { passphrase }) : await invoke<string | null>("keyvault_setup"),
      ),
  };
}

/** The SwiftUI host: Touch ID setup only; it answers `{ recoveryKey }`. */
export function webkitKeyvaultSetupOps(request: Request, nativeOnly: () => Error): Handlers {
  return {
    "keyvault.setup": async ({ passphrase }) => {
      if (passphrase) throw nativeOnly();
      return setupResultFromWire(await request<unknown>("keyvault.setup"));
    },
  };
}
