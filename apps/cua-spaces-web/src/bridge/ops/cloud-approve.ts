// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * "Approve AWS for your agent?": a person approves or denies what their
 * agent set up in a cloud (a link, or a raise of its limits). The daemon's
 * `cloud_approve` tool takes it to Cua's issuer
 * (`POST /v1/connections/{id}/approve` or `/deny`). It is a person-only
 * tool: no MCP server lists or runs it, and the daemon refuses an agent
 * caller, so only the apps reach it, through the daemon's tool call. Each
 * cloud in `cloud_status` says whether this caller may approve it
 * (`can_approve`); the page offers Approve and Deny only then.
 *
 * | Operation | Electron | SwiftUI | Tauri |
 * |---|---|---|---|
 * | `clouds.approve {provider, approve}` | `cloud_approve` | `cloud_approve` | `cloud_tool` `cloud_approve` |
 */

import type { OpCoverage } from "../coverage";

export interface CloudApproveOps {
  /** Approves (or, with `approve: false`, denies) what an agent set up in `provider`. */
  "clouds.approve": { args: { provider: string; approve: boolean }; result: null };
}

export const CLOUD_APPROVE_OPERATIONS = ["clouds.approve"] as const satisfies readonly (keyof CloudApproveOps)[];

export const CLOUD_APPROVE_COVERAGE = {
  "clouds.approve": { webkit: { methods: ["clouds.approve"] }, tauri: ["cloud_tool"] },
} as const satisfies Record<keyof CloudApproveOps, OpCoverage>;

type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

/** Tauri: the daemon's `cloud_approve` through `cloud_tool`. */
export function cloudApproveTauriOps(invoke: Invoke) {
  return {
    "clouds.approve": async ({ provider, approve }: CloudApproveOps["clouds.approve"]["args"]) => {
      await invoke("cloud_tool", { tool: "cloud_approve", args: { provider, deny: !approve } });
      return null;
    },
  };
}

/** WebKit (SwiftUI and Electron): the host's `clouds.approve` method. */
export function cloudApproveWebkitOps(request: <T>(method: "clouds.approve", args: Record<string, unknown>) => Promise<T>) {
  return {
    "clouds.approve": async ({ provider, approve }: CloudApproveOps["clouds.approve"]["args"]) => {
      await request("clouds.approve", { provider, approve });
      return null;
    },
  };
}

/** Arguments that take it down its usual path (coverage tests). */
export const CLOUD_APPROVE_ARGS = {
  "clouds.approve": { provider: "aws", approve: true },
};
