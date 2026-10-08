// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Keyvault page beyond lock and unlock: what the SwiftUI Keyvault does
 * with the same views the core builds (`keyvault.sidebar`, `keyvault.list`,
 * `keyvault.vaultView`).
 *
 * | Operation | SwiftUI |
 * |---|---|
 * | `keyvault.showItems` | `KeyvaultModel.showItems` (the daemon asks for Touch ID, then lists names) |
 * | `keyvault.delete {itemIds}` | `requestDelete`, confirmed in a native alert built from `kvDeleteConfirm`, then `confirmDelete` (live copies in Spaces are wiped) |
 * | `keyvault.run {command}` | `KeyvaultModel.run`: an Access row's own command (revoke a grant, remove a rule, wipe a copy) |
 * | `keyvault.dismiss {imports}` | `KeyvaultModel.dismiss`: hides those copies from the notch; never revokes or wipes |
 *
 * The overview carries `dismissed` (the copies hidden from the notch), which
 * the core's `keyvault.signedInSpaces` takes. The Tauri app has its own
 * Keyvault screen and no commands for these; the Electron shell answers the
 * SwiftUI host's methods.
 */

import type { OpCoverage } from "../coverage";
import type { KeyvaultOverview, KvCommand } from "../contracts/keyvault";
import type { OpArgs, OpResult } from "../protocol";

/** What an Access row sends: revoke a grant, remove a rule, wipe a delivered copy. */
export type KvAccessCommand = Extract<KvCommand, { type: "revoke-grant" | "remove-rule" | "release" }>;

declare module "../protocol" {
  interface HostOperations {
    /** Shows the items' names (the daemon asks for Touch ID); the overview then has them. */
    "keyvault.showItems": { args: Record<string, never>; result: KeyvaultOverview };
    /** Deletes items and wipes their live copies, after the host's own confirmation (`cancelled` if declined). */
    "keyvault.delete": { args: { itemIds: string[] }; result: null };
    /** An Access row's command. */
    "keyvault.run": { args: { command: KvAccessCommand }; result: null };
    /** Hides delivered copies (import ids) from the notch; nothing is revoked or wiped. */
    "keyvault.dismiss": { args: { imports: string[] }; result: null };
  }
}

export const KEYVAULT_MANAGE_OPERATIONS = ["keyvault.showItems", "keyvault.delete", "keyvault.run", "keyvault.dismiss"] as const;
export type KeyvaultManageOp = (typeof KEYVAULT_MANAGE_OPERATIONS)[number];

export const KEYVAULT_MANAGE_WEBKIT_METHODS = KEYVAULT_MANAGE_OPERATIONS;

const TAURI = "the Tauri app has its own Keyvault screen and no command for this";

const row = <K extends KeyvaultManageOp>(op: K): OpCoverage => ({
  webkit: { methods: [op] },
  tauri: [],
  unsupported: { tauri: TAURI },
});

export const KEYVAULT_MANAGE_COVERAGE = {
  "keyvault.showItems": row("keyvault.showItems"),
  "keyvault.delete": row("keyvault.delete"),
  "keyvault.run": row("keyvault.run"),
  "keyvault.dismiss": row("keyvault.dismiss"),
} as const satisfies Record<KeyvaultManageOp, OpCoverage>;

export const KEYVAULT_MANAGE_ARGS: { [K in KeyvaultManageOp]: OpArgs<K> } = {
  "keyvault.showItems": {},
  "keyvault.delete": { itemIds: ["kv-linear"] },
  "keyvault.run": { command: { type: "revoke-grant", id: "grant-1" } },
  "keyvault.dismiss": { imports: ["imp-1"] },
};

/** No Tauri command answers these (`TAURI_UNSUPPORTED`). */
export const KEYVAULT_MANAGE_TAURI_UNSUPPORTED = KEYVAULT_MANAGE_OPERATIONS;

type Handlers = { [K in KeyvaultManageOp]: (args: OpArgs<K>) => Promise<OpResult<K>> };
type Request = <T>(method: KeyvaultManageOp, args?: Record<string, unknown>) => Promise<T>;

/** The SwiftUI host answers each with the Keyvault (`keyvault.get`'s shape), or the action's error. */
export function webkitKeyvaultManageOps(request: Request, toOverview: (wk: never) => KeyvaultOverview): Handlers {
  return {
    "keyvault.showItems": async () => toOverview(await request<never>("keyvault.showItems")),
    "keyvault.delete": async ({ itemIds }) => {
      await request("keyvault.delete", { ids: itemIds });
      return null;
    },
    "keyvault.run": async ({ command }) => {
      await request("keyvault.run", { command });
      return null;
    },
    "keyvault.dismiss": async ({ imports }) => {
      await request("keyvault.dismiss", { imports });
      return null;
    },
  };
}
