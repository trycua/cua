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
 * | `relay.usage` | the host method `relay.usage` | the host method `relay.usage` | unsupported |
 *
 * Both native hosts read it through the Spaces app export's
 * `app_relay_usage` (the relay's `GET /v1/usage` with the account's token,
 * as `cua usage` reads it). The row and the notice are the app core's
 * `relayUsage.view` (`derive.ts`), as the SwiftUI app draws them.
 *
 * Null: signed out, or a relay that meters nothing. Where unsupported, the
 * page shows nothing about it.
 */

import type { OpCoverage } from "../coverage";

/** `cua_spaces_app_core::relay_usage::RelayUsage`. */
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

/** `relay_usage::RelayAllowanceNotice`: the line above every page past the allowance. */
export interface RelayAllowanceNotice {
  text: string;
  /** "Upgrade". */
  upgrade: string;
  /** What Upgrade opens (https); null: no Upgrade button. */
  upgradeUrl: string | null;
  /** "Dismiss". */
  dismiss: string;
  /** A dismissal holds for this month and these reasons. */
  dismissKey: string;
  /** `warn`: 80% of the relay hours (a prompt, held back during a live
   * session or an agent run). `limit`: the allowance is used (always shown). */
  level: "warn" | "limit";
  /** For usage data: the cap (`sessions`, `hours`; `none` for the 80%
   * heads-up). Absent from cores before it. */
  kind?: string;
  /** For usage data: `free`, `pro`, `teams` or `unknown`. */
  plan?: string;
}

/** `relay_usage::PlanLink`: a button that opens a page on the account app. */
export interface PlanLink {
  label: string;
  url: string;
}

/** `relay_usage::PlanCard`: Settings' plan card, always there. */
export interface PlanCard {
  plan: "free" | "pro" | "teams" | string;
  /** "Free plan". */
  label: string;
  detail: string;
  /** "Get Pro". */
  primary: PlanLink | null;
  /** "Start a team". */
  secondary: PlanLink | null;
}

/** `relay_usage::RelayUsageView`: Settings' row and the notice. */
export interface RelayUsageView {
  /** "Remote use". */
  label: string;
  /** "Live sessions 1 of 2 · Remote hours 6.5 of 20 this month". */
  line: string;
  /** "Free plan". */
  plan: string;
  notice: RelayAllowanceNotice | null;
  planCard: PlanCard;
}

export interface RelayUsageOps {
  "relay.usage": { args: Record<string, never>; result: RelayUsage | null };
}

export const RELAY_USAGE_OPERATIONS = ["relay.usage"] as const satisfies readonly (keyof RelayUsageOps)[];

export const RELAY_USAGE_COVERAGE = {
  "relay.usage": {
    webkit: { methods: ["relay.usage"] },
    tauri: [],
    unsupported: {
      tauri: "the Tauri app does not read the relay allowance",
    },
  },
} as const satisfies Record<keyof RelayUsageOps, OpCoverage>;

/** Arguments that take it down its usual path (coverage tests). */
export const RELAY_USAGE_ARGS = { "relay.usage": {} };
