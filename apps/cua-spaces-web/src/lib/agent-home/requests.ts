// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The agent's requests for abilities, read from its home's thread. An agent
 * asks the way it asks for anything else: an approval entry whose action is
 * `ability:<id>` (see `prompt.ts`). The person answers it on the
 * standard approval card in the home Space's normal thread (`answer.ts`); an answer to "list all" or "use this machine" is remembered for
 * that agent (`grants.ts`) when they say so, and a remembered one is
 * answered at once.
 */

import type { ThreadEntry } from "@/bridge/ops/thread";

import type { AbilityId, AgentGrants } from "./abilities";
import { abilityOfAction } from "./prompt";

export interface AbilityRequest {
  /** The approval entry's seq: what an answer refers to. */
  seq: number;
  ability: AbilityId;
  /** The agent's own words for what it wants ("the Mac mini"). */
  detail: string;
  tsMs: number;
}

/** The ability requests nobody has answered yet, oldest first. */
export function openAbilityRequests(entries: readonly ThreadEntry[]): AbilityRequest[] {
  const answered = new Set(entries.filter((e) => e.kind === "answer" && e.answer).map((e) => e.answer!.approval));
  const out: AbilityRequest[] = [];
  for (const e of entries) {
    if (e.kind !== "approval" || !e.approval || answered.has(e.seq)) continue;
    const ability = abilityOfAction(e.approval.action);
    if (ability) out.push({ seq: e.seq, ability, detail: e.approval.details.trim(), tsMs: e.tsMs });
  }
  return out.sort((a, b) => a.seq - b.seq);
}

/** Whether what the agent holds already covers this request (so no question is needed). */
export function covered(req: AbilityRequest, grants: AgentGrants, machineIds: ReadonlyMap<string, string>): boolean {
  if (req.ability === "spaces.spawn") return true;
  if (req.ability === "spaces.list") return grants.listAll;
  // `machines.use`: the machine the agent named, matched by name or id.
  const named = req.detail.toLowerCase();
  for (const [id, name] of machineIds) {
    if (grants.machines.includes(id) && (named.includes(name.toLowerCase()) || named.includes(id.toLowerCase()))) return true;
  }
  return false;
}

/** The question the person is asked, in plain words. */
export function askText(req: AbilityRequest, machineName: string | null): { title: string; detail: string; allow: string } {
  switch (req.ability) {
    case "spaces.list":
      return {
        title: "Let this agent see all your Spaces?",
        detail: "Until you say yes it only sees the Spaces you @mention. It can look at them, not change them.",
        allow: "Allow",
      };
    case "machines.use":
      return {
        title: machineName ? `Let this agent use ${machineName}?` : "Let this agent use this computer?",
        detail: req.detail || "It will put work on that computer.",
        allow: "Allow",
      };
    default:
      return { title: "Let this agent make a Space?", detail: req.detail, allow: "Allow" };
  }
}
