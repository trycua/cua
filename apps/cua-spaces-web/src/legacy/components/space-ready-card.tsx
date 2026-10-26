// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";
import { XIcon } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { useAgents, useFeatureRecorder, type AgentSetupRow, type Space } from "@/bridge";
import { CuaKey } from "@/components/cua-key";
import { suggestedTask, type SuggestedTask } from "@/components/first-run-guide";
import { Button } from "@/components/ui/button";

const KEY = "cua.agentOffer";

type Outcome = "dismissed" | "done";

const read = (): Outcome | null => {
  try {
    const v = globalThis.localStorage?.getItem(KEY);
    return v === "dismissed" || v === "done" ? v : null;
  } catch {
    return null;
  }
};

/**
 * The offer to connect an agent after the first Space is ready. It stays
 * until the person closes it or connects an agent, then never comes back.
 * Events: `agent_offer_shown` (once per install), `agent_offer_clicked`,
 * `agent_offer_completed` (an agent got connected, or its request was copied).
 */
export function useAgentOffer(eligible: boolean) {
  const record = useFeatureRecorder();
  const [outcome, setOutcome] = useState<Outcome | null>(read);
  const visible = eligible && outcome === null;
  useEffect(() => {
    if (!visible) return;
    try {
      if (globalThis.localStorage?.getItem(`${KEY}.shown`)) return;
      globalThis.localStorage?.setItem(`${KEY}.shown`, "1");
    } catch {
      // Not kept: it may count again on the next launch.
    }
    record("agent_offer_shown");
  }, [visible, record]);
  const finish = useCallback((next: Outcome) => {
    try {
      globalThis.localStorage?.setItem(KEY, next);
    } catch {
      // Not kept: it may show again on the next launch.
    }
    setOutcome(next);
  }, []);
  return {
    visible,
    clicked: () => record("agent_offer_clicked"),
    completed: () => {
      if (!visible) return;
      record("agent_offer_completed");
      finish("done");
    },
    dismiss: () => finish("dismissed"),
  };
}

/** "Claude Code", "Claude Code and Codex", "Claude Code, Codex and Hermes". */
export function listNames(names: string[]): string {
  return names.length < 2 ? (names[0] ?? "") : `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/**
 * The moment a user's first Space is ready (the guided first run's success
 * state, CUA-2062): it says so, names the agents found on this computer, and
 * offers one real task to try (`suggestedTask`). "Try this task" opens the
 * run dialog with that request (which connects an agent first if needed);
 * "Open" goes to the Space. Add computer and Share stay in the checklist
 * below it, and a sign-in prompt may sit under it.
 */
export function SpaceReadyCard({ space, onRun, onDismiss }: { space: Space; onRun: (task: SuggestedTask) => void; onDismiss?: () => void }) {
  const { agentSetup } = useAgents();
  const navigate = useNavigate();
  const [rows, setRows] = useState<AgentSetupRow[] | null>(null);
  useEffect(() => {
    let live = true;
    agentSetup().then(
      (r) => live && setRows(r),
      () => live && setRows([]),
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const task = suggestedTask(space);
  const found = (rows ?? []).filter((r) => r.installed);
  const line = found.length
    ? `We found ${listNames(found.map((r) => r.name))} on this computer. Give one a task and watch it work in ${space.name}.`
    : "Connect an agent such as Claude Code or Codex, give it a task, and watch it work in this Space.";

  return (
    <section data-space-ready="" aria-labelledby="space-ready-title" className="mb-6 rounded-xl border bg-card px-5 py-4 shadow-xs">
      <div className="flex items-start gap-4">
        <CuaKey size={40} className="size-10" />
        <div className="min-w-0 flex-1">
          <h2 id="space-ready-title" className="text-[14px] font-semibold">
            Your first Space is ready
          </h2>
          <p className="mt-0.5 text-xs text-muted-foreground">
            {space.name} is a fresh computer, set up and running. {rows === null ? "Looking for your agents…" : line}
          </p>
        </div>
        {onDismiss ? (
          <Button data-space-ready-dismiss="" size="icon-sm" variant="ghost" aria-label="Not now" onClick={onDismiss}>
            <XIcon className="size-3.5" />
          </Button>
        ) : null}
      </div>
      <div data-suggested-task="" className="mt-4 flex items-center gap-4 rounded-lg border bg-muted/40 px-4 py-3">
        <div className="min-w-0 flex-1">
          <p className="text-2xs font-medium text-muted-foreground">Try this first</p>
          <p className="mt-0.5 text-[13px] font-medium">{task.title}</p>
          <p className="mt-0.5 text-xs text-muted-foreground">{task.why}</p>
        </div>
        <Button variant="outline" data-space-ready-open="" onClick={() => void navigate({ to: "/spaces/$spaceId", params: { spaceId: space.id } })}>
          Open {space.name}
        </Button>
        <Button data-space-ready-run="" disabled={rows === null} onClick={() => onRun(task)}>
          Try this task
        </Button>
      </div>
    </section>
  );
}
