// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Answers for an agent's ability requests, in the Space's normal thread.
 *
 * The agent asks with a standard approval whose action is `ability:<id>`
 * (`prompt.ts`); the thread draws it as any agent's approval card, in plain
 * words (`requests.ts` `askText`). This hook sits between that card and the
 * thread's `answer`: allowing remembers the grant for that agent, tells this
 * machine's daemon so the running home takes it at once, and a request the
 * agent already holds is answered without asking.
 */

import { useCallback, useEffect, useMemo, useRef } from "react";

import { useBridge, useMachines, type ThreadItem } from "@/bridge";
import { toastError } from "@/components/ui/toast";
import type { Roster } from "@/components/space-thread/roster";

import { allowListAll, allowMachine, grantsFor, homeGrantsArg } from "./grants";
import { isHomeSpaceId } from "./home-space";
import { abilityOfAction } from "./prompt";
import { covered, type AbilityRequest } from "./requests";

type ApprovalItem = Extract<ThreadItem, { type: "approval" }>;

/** The ability an approval item asks for, and who asked. */
function requestOf(item: ApprovalItem, roster: Pick<Roster, "agents">): { req: AbilityRequest; agent: string } | null {
  const action = item.entry.approval?.action;
  const ability = action ? abilityOfAction(action) : null;
  if (!ability) return null;
  const asker = item.entry.author.kind === "agent" ? roster.agents.find((a) => a.id === item.entry.author.id) : undefined;
  return {
    req: { seq: item.entry.seq, ability, detail: (item.entry.approval?.details ?? "").trim(), tsMs: item.entry.tsMs },
    agent: asker?.harness ?? "",
  };
}

/** The machine a request names (by name or id in the agent's words), among the person's machines. */
function machineFor(req: AbilityRequest, machines: ReadonlyMap<string, string>): [string, string] | null {
  const named = req.detail.toLowerCase();
  return [...machines].find(([id, n]) => named.includes(n.toLowerCase()) || named.includes(id.toLowerCase())) ?? null;
}

/**
 * `answer` for the thread's approval cards: the thread's own answer, with an
 * ability request's grant kept first. Also answers, once, the open requests
 * that what the agent already holds covers.
 */
export function useAbilityAnswer(
  spaceId: string,
  items: readonly ThreadItem[],
  roster: Pick<Roster, "agents">,
  answer: (approval: number, approve: boolean) => Promise<void>,
): (approval: number, approve: boolean) => Promise<void> {
  const { data } = useBridge();
  const { data: machineRows } = useMachines();
  const machines = useMemo(() => new Map((machineRows ?? []).map((m) => [m.id, m.name] as const)), [machineRows]);
  const home = isHomeSpaceId(spaceId);
  const asked = useRef(new Set<number>());

  const decide = useCallback(
    async (approval: number, approve: boolean) => {
      const item = items.find((i): i is ApprovalItem => i.type === "approval" && i.entry.seq === approval);
      const found = item ? requestOf(item, roster) : null;
      if (found && approve) {
        const { req, agent } = found;
        if (req.ability === "spaces.list") allowListAll(agent);
        if (req.ability === "machines.use") {
          const m = machineFor(req, machines);
          if (m) allowMachine(agent, m[0]);
        }
        // The running home's grants change now: this machine's daemon reads them each time the home asks.
        if (home && req.ability !== "spaces.spawn") {
          await data?.call("spaces.setAccess", { spaceId, home: homeGrantsArg(grantsFor(agent), machines) }).catch(toastError("Couldn't let it through yet"));
        }
      }
      asked.current.add(approval);
      await answer(approval, approve);
    },
    [items, roster, machines, home, data, spaceId, answer],
  );

  useEffect(() => {
    // What it already holds is answered at once; the rest waits for the person.
    for (const item of items) {
      if (item.type !== "approval" || item.answer || (item.entry.approval?.state ?? "open") !== "open") continue;
      const found = requestOf(item, roster);
      if (!found || asked.current.has(found.req.seq)) continue;
      if (!covered(found.req, grantsFor(found.agent), machines)) continue;
      asked.current.add(found.req.seq);
      void answer(found.req.seq, true).catch(() => asked.current.delete(found.req.seq));
    }
  }, [items, roster, machines, answer]);

  return decide;
}
