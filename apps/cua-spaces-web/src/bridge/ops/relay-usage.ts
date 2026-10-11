// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The account's relay allowance this month (`cua usage`). A live session is
 * a connection to one of your remote machines through the hosted relay;
 * relay hours are the time with at least one. Nothing stops at the limit:
 * past it, the page says so and offers the upgrade.
 *
 * | Operation | Electron | SwiftUI | Tauri |
 * |---|---|---|---|
 * | `relay.usage` | the host method `relay.usage`: the relay's `GET /v1/usage` with the account's token (`src/bridge/relay-usage.ts`) | unsupported | unsupported |
 *
 * Null: signed out, or a relay that meters nothing. Where unsupported, the
 * page shows nothing about it.
 */

import { UnsupportedOperationError, type BridgeMode } from "../adapter";
import type { OpCoverage } from "../coverage";

/** `cua_host::RelayUsage`, camelCase. */
export interface RelayUsage {
  metered: boolean;
  /** The month counted (`2026-10`). */
  period: string;
  /** `free`, `pro`, `team`, ... */
  plan: string;
  /** The team whose allowance is pooled. */
  team: { id: string; seats: number } | null;
  liveSessions: number;
  /** Null: no limit. */
  liveSessionsLimit: number | null;
  /** The team's, when pooled. */
  hoursUsed: number;
  hoursLimit: number | null;
  myHoursUsed: number;
  softLimitReached: boolean;
  /** What is past the allowance: `sessions` and/or `hours`. */
  reasons: string[];
  upgradeUrl: string | null;
}

export interface RelayUsageOps {
  "relay.usage": { args: Record<string, never>; result: RelayUsage | null };
}

export const RELAY_USAGE_OPERATIONS = ["relay.usage"] as const satisfies readonly (keyof RelayUsageOps)[];

export const RELAY_USAGE_COVERAGE = {
  "relay.usage": {
    webkit: { methods: [] },
    electron: { methods: ["relay.usage"] },
    tauri: [],
    unsupported: {
      webkit: "the Mac app does not read the relay allowance yet",
      tauri: "the Tauri app does not read the relay allowance",
    },
  },
} as const satisfies Record<keyof RelayUsageOps, OpCoverage>;

/** Hosts that do not read the allowance. */
export function unsupportedRelayUsageOps(mode: BridgeMode) {
  return {
    "relay.usage": (): Promise<RelayUsage | null> => Promise.reject(new UnsupportedOperationError(mode, "relay.usage")),
  };
}

/** Arguments that take it down its usual path (coverage tests). */
export const RELAY_USAGE_ARGS = { "relay.usage": {} };
