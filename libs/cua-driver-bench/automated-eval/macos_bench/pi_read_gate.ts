// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// pi extension for the pi-* bench arms (pi_arms.py). The Claude Code arms load the skill with `Skill` and every
// `Read` is denied (permission mode dontAsk, only mcp__cua allowed), so `read` here may open the skill's SKILL.md
// and nothing else. CDB_PI_READ_ALLOW names that file.
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
  pi.on("tool_call", async (event: any) => {
    if (event.toolName !== "read") return undefined;
    const path = real(resolve(process.cwd(), String(event.input?.path ?? "")));
    if (allowed && path === allowed) return undefined;
    return { block: true, reason: "Permission to use read has been denied; only the skill's SKILL.md is readable." };
  });
}
