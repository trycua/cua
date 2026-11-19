// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The CI Runners page's operations: GitHub Actions jobs on your own
 * machines (`cua ci`). Each is one `cua ci app <op>` run by the host: the
 * Mac app and the Electron app start the bundled `cua` with a fixed
 * operation name, pass the arguments as one JSON object on stdin and read
 * one JSON line back (`libs/cua/crates/cua-cli/src/ci_app.rs`). What may be
 * asked, what is checked and where a credential goes are decided there,
 * once; the hosts only run it. A GitHub token crosses the bridge once, in
 * `ci.connectToken`'s arguments, and never comes back.
 *
 * | Operation | `cua ci app` | What it does |
 * |---|---|---|
 * | `ci.overview {live?}` | `overview` | The page's data (`cua ci overview`); `live` also asks the service for the repositories |
 * | `ci.connect {githubOrg?}` | `connect` | The hosted GitHub app, one step per call: `waiting` with the page to open, then `connected` |
 * | `ci.connectToken {org, token?}` | `connect_token` | A pool from a GitHub token (the one given, else the one kept in Settings, Connections) |
 * | `ci.connectCancel` | `connect_cancel` | Forgets a GitHub install in progress |
 * | `ci.repositories {allow?, remove?}` | `repositories` | Which repositories may run jobs |
 * | `ci.goal {target, goal}` | `goal` | Cheapest, balanced or fastest |
 * | `ci.log {jobKey, pool, slot}` | `log` | A job's own log, plain sentences |
 *
 * Tauri has no host for these: it rejects as unsupported.
 */

import { UnsupportedOperationError, type BridgeMode } from "../adapter";
import type { OpCoverage } from "../coverage";
import type { CiConnectAnswer, CiGoal, CiLogLine, CiOverview } from "../contracts/ci-runners";

export interface CiOps {
  "ci.overview": { args: { live?: boolean }; result: CiOverview };
  "ci.connect": { args: { githubOrg?: string }; result: CiConnectAnswer };
  "ci.connectToken": { args: { org: string; token?: string }; result: CiConnectAnswer };
  "ci.connectCancel": { args: Record<string, never>; result: null };
  "ci.repositories": { args: { allow?: string[]; remove?: string[] }; result: { changed?: string[] } };
  "ci.goal": { args: { target: string; goal: CiGoal }; result: { goal: string; applied: boolean } };
  "ci.log": { args: { jobKey: string; pool: string; slot: string }; result: { lines: CiLogLine[] } };
}

export const CI_OPERATIONS = [
  "ci.overview",
  "ci.connect",
  "ci.connectToken",
  "ci.connectCancel",
  "ci.repositories",
  "ci.goal",
  "ci.log",
] as const satisfies readonly (keyof CiOps)[];

/** The host methods (`cua ci app` operations by the same name). */
export const CI_WEBKIT_METHODS = CI_OPERATIONS;

const TAURI = "the Tauri app has no host for `cua ci app`";

export const CI_COVERAGE = {
  "ci.overview": { webkit: { methods: ["ci.overview"] }, tauri: [], unsupported: { tauri: TAURI } },
  "ci.connect": { webkit: { methods: ["ci.connect"] }, tauri: [], unsupported: { tauri: TAURI } },
  "ci.connectToken": { webkit: { methods: ["ci.connectToken"] }, tauri: [], unsupported: { tauri: TAURI } },
  "ci.connectCancel": { webkit: { methods: ["ci.connectCancel"] }, tauri: [], unsupported: { tauri: TAURI } },
  "ci.repositories": { webkit: { methods: ["ci.repositories"] }, tauri: [], unsupported: { tauri: TAURI } },
  "ci.goal": { webkit: { methods: ["ci.goal"] }, tauri: [], unsupported: { tauri: TAURI } },
  "ci.log": { webkit: { methods: ["ci.log"] }, tauri: [], unsupported: { tauri: TAURI } },
} as const satisfies Record<keyof CiOps, OpCoverage>;

/** A host that can't run CI operations. */
export function unsupportedCiOps(mode: BridgeMode) {
  const no = (op: keyof CiOps) => (): Promise<never> => Promise.reject(new UnsupportedOperationError(mode, op));
  return {
    "ci.overview": no("ci.overview"),
    "ci.connect": no("ci.connect"),
    "ci.connectToken": no("ci.connectToken"),
    "ci.connectCancel": no("ci.connectCancel"),
    "ci.repositories": no("ci.repositories"),
    "ci.goal": no("ci.goal"),
    "ci.log": no("ci.log"),
  };
}

/** Arguments that take each down its usual path (coverage tests). */
export const CI_ARGS = {
  "ci.overview": { live: true },
  "ci.connect": {},
  "ci.connectToken": { org: "acme" },
  "ci.connectCancel": {},
  "ci.repositories": { allow: ["acme/api"] },
  "ci.goal": { target: "aws", goal: "fastest" as const },
  "ci.log": { jobKey: "9911/298", pool: "hosted", slot: "s7" },
};
