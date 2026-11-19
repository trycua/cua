// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The risky-action question (`cua config set nudges.risky_actions on`): the
 * requests `cua mcp` is holding for an answer, and the answer.
 *
 * | Operation | Electron | SwiftUI | Tauri |
 * |---|---|---|---|
 * | `nudges.pending` | the host method `nudges.pending` | the host method `nudges.pending` | unsupported |
 * | `nudges.answer` | the host method `nudges.answer` | the host method `nudges.answer` | unsupported |
 *
 * Both native hosts read `$CUA_HOME/nudges/<id>.json` and write the answer
 * next to it (`<id>.answer`: `separate` or `here`); `cua mcp` waits for it
 * (`cua-cli/src/risky.rs`), and runs the call here when none comes. Where
 * unsupported, the question never shows and every call runs as before.
 */

import type { OpCoverage } from "../coverage";

/** `nudges::RiskyRequest`: one request file. */
export interface RiskyRequest {
  id: string;
  /** "Claude Code". */
  agent: string;
  /** "empty the Trash". */
  action: string;
  atMs: number;
  /** When it runs here without an answer. */
  expiresMs: number;
}

export type RiskyChoice = "separate" | "here";

export interface NudgesOps {
  /** Requests waiting for an answer, oldest first. */
  "nudges.pending": { args: Record<string, never>; result: RiskyRequest[] };
  "nudges.answer": { args: { id: string; choice: RiskyChoice }; result: null };
}

export const NUDGES_OPERATIONS = ["nudges.pending", "nudges.answer"] as const satisfies readonly (keyof NudgesOps)[];

const TAURI = "the Tauri app does not answer the risky-action question (calls run as before)";

export const NUDGES_COVERAGE = {
  "nudges.pending": { webkit: { methods: ["nudges.pending"] }, tauri: [], unsupported: { tauri: TAURI } },
  "nudges.answer": { webkit: { methods: ["nudges.answer"] }, tauri: [], unsupported: { tauri: TAURI } },
} as const satisfies Record<keyof NudgesOps, OpCoverage>;

/** Arguments that take each down its usual path (coverage tests). */
export const NUDGES_ARGS = { "nudges.pending": {}, "nudges.answer": { id: "risky-1-1", choice: "here" as RiskyChoice } };

/** A request id as `cua mcp` writes it: no path can hide in one. */
export function validRequestId(id: string): boolean {
  return /^risky-[0-9]+-[0-9]+$/.test(id);
}
