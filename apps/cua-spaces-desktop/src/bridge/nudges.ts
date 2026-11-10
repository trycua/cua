// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The risky-action question's files (`cua config set nudges.risky_actions
// on`): `cua mcp` writes `$CUA_HOME/nudges/<id>.json` and waits for
// `<id>.answer` (`separate` or `here`); with no answer in time it runs the
// call here (`libs/cua/crates/cua-cli/src/risky.rs`). The same reads and
// writes as the SwiftUI host's `NudgeRequests`.
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

import { cuaHome } from "../model/daemon";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";

const ID = /^risky-[0-9]+-[0-9]+$/;

export interface RiskyRequest {
  id: string;
  agent: string;
  action: string;
  atMs: number;
  expiresMs: number;
}

/** Requests still waiting (no answer, not past their deadline), oldest first. */
export function pendingRequests(dir: string, nowMs = Date.now()): RiskyRequest[] {
  let names: string[];
  try {
    names = readdirSync(dir);
  } catch {
    return [];
  }
  const out: RiskyRequest[] = [];
  for (const name of names) {
    if (!name.endsWith(".json")) continue;
    const id = name.slice(0, -5);
    if (!ID.test(id) || existsSync(path.join(dir, `${id}.answer`))) continue;
    try {
      const r = JSON.parse(readFileSync(path.join(dir, name), "utf8")) as Partial<RiskyRequest>;
      if (typeof r.expiresMs !== "number" || r.expiresMs <= nowMs) continue;
      out.push({ id, agent: String(r.agent ?? ""), action: String(r.action ?? ""), atMs: Number(r.atMs ?? 0), expiresMs: r.expiresMs });
    } catch {
      // Being written, or not ours: skip it.
    }
  }
  return out.sort((a, b) => a.atMs - b.atMs);
}

/** Answers a waiting request; one that is gone (it ran here already) is left alone. */
export function answerRequest(dir: string, id: string, choice: string): void {
  if (!ID.test(id) || (choice !== "separate" && choice !== "here")) return;
  if (!existsSync(path.join(dir, `${id}.json`))) return;
  writeFileSync(path.join(dir, `${id}.answer`), choice);
}

export function nudgesMethods(_ctx: BridgeContext, dir: () => string = () => path.join(cuaHome(), "nudges")): Handlers {
  return {
    "nudges.pending": async () => pendingRequests(dir()),
    "nudges.answer": async (args) => {
      const { id, choice } = args;
      if (typeof id !== "string" || typeof choice !== "string") throw Failure.badArgs("id: string, choice: separate or here");
      answerRequest(dir(), id, choice);
      return null;
    },
  };
}
