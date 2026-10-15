// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Machines page's own actions: rename a machine, and remove one from
 * the account. What each one does to which machine is the app core's
 * (`machines_view::MachinesViewRename`, `MachinesViewRemove`); the page
 * passes it on.
 *
 * | Operation | Electron | SwiftUI | Tauri |
 * |---|---|---|---|
 * | `machines.rename` | the host method `machines.rename` | the host method `machines.rename` | unsupported |
 * | `machines.remove` | the host method `machines.remove` | the host method `machines.remove` | unsupported |
 *
 * `machines.rename {id, name}`: `id` is a relay machine of yours, or
 * `this-mac` for this machine (the host renames its own registration).
 * Both hosts call the SDK's `Devices.rename_machine` from this enrolled
 * device; this machine's host setup takes the new name too. A device of its
 * own is renamed with `devices.rename`.
 *
 * `machines.remove {machine, device}`: `machine` leaves the relay directory
 * (`Spaces.relay_unregister("relay:<machine>")`, refused while it is
 * online), then `device` is revoked. This machine's own host setup is
 * removed with `host.action {action: "remove"}`.
 */

import type { OpCoverage } from "../coverage";

export interface MachinesOps {
  "machines.rename": { args: { id: string; name: string }; result: null };
  "machines.remove": { args: { machine?: string | null; device?: string | null }; result: null };
}

export const MACHINES_OPERATIONS = ["machines.rename", "machines.remove"] as const satisfies readonly (keyof MachinesOps)[];

export const MACHINES_COVERAGE = {
  "machines.rename": {
    webkit: { methods: ["machines.rename"] },
    tauri: [],
    unsupported: { tauri: "the Tauri app has no Machines page actions" },
  },
  "machines.remove": {
    webkit: { methods: ["machines.remove"] },
    tauri: [],
    unsupported: { tauri: "the Tauri app has no Machines page actions" },
  },
} as const satisfies Record<keyof MachinesOps, OpCoverage>;

/** Arguments that take each down its usual path (coverage tests). */
export const MACHINES_ARGS = {
  "machines.rename": { id: "m-studio", name: "Studio" },
  "machines.remove": { machine: "m-old", device: "dev_old" },
};
