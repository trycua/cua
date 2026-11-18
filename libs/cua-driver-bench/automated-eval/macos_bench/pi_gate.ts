// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// pi extension for the pi-* bench arms (pi_arms.py), giving pi the limits Claude Code's own flags give the cc-* arms.
// Read: the Claude Code arms load the skill with `Skill`, and every `Read` is denied (permission mode dontAsk, only
// mcp__cua allowed), so `read` may open the skill's SKILL.md (CDB_PI_READ_ALLOW) and nothing else.
// Turns: like --max-turns, the CDB_PI_MAX_TURNS-th assistant turn may still run its tools; the next turn is aborted
// before its model request, and no tool call runs after the cap.
import { realpathSync } from "node:fs";
import { resolve } from "node:path";

function real(path: string): string {
  try {
    return realpathSync(path);
  } catch {
    return path;
  }
}

export default function (pi: any) {
  const allowed = process.env.CDB_PI_READ_ALLOW ? real(process.env.CDB_PI_READ_ALLOW) : undefined;
  const maxTurns = Number(process.env.CDB_PI_MAX_TURNS || "0");
  let turns = 0;
  pi.on("turn_start", async (_event: any, ctx: any) => {
    turns += 1;
    if (maxTurns > 0 && turns > maxTurns) ctx.abort();
  });
  pi.on("tool_call", async (event: any) => {
    if (maxTurns > 0 && turns > maxTurns) {
      return { block: true, reason: "The turn limit is reached.", terminate: true };
    }
    if (event.toolName !== "read") return undefined;
    const path = real(resolve(process.cwd(), String(event.input?.path ?? "")));
    if (allowed && path === allowed) return undefined;
    return { block: true, reason: "Permission to use read has been denied; only the skill's SKILL.md is readable." };
  });
}
