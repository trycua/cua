// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { BotIcon, XIcon } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { useAgents, useFeatureRecorder, type AgentSetupRow, type Space } from "@/bridge";
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
 * The moment a user's first Space is ready: let an agent use it. Shows the
 * agents found on this computer and one action. Add computer and Share stay
 * in the checklist below it.
 */
export function SpaceReadyCard({ space, onRun, onDismiss }: { space: Space; onRun: () => void; onDismiss?: () => void }) {
  const { agentSetup } = useAgents();
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

  const found = (rows ?? []).filter((r) => r.installed);
  const first = found.find((r) => r.configured) ?? found[0];
  const action = !first ? "Show me how" : first.configured ? `Start with ${first.name}` : `Connect ${first.name}`;
  const line = found.length
    ? `We found ${listNames(found.map((r) => r.name))} on this computer. Let one work in ${space.name} while you watch.`
    : "Connect an agent such as Claude Code or Codex, give it a task, and watch it work on this computer.";

  return (
    <section data-space-ready="" aria-labelledby="space-ready-title" className="mb-6 flex items-center gap-4 rounded-xl border bg-card px-5 py-4 shadow-xs">
      <span aria-hidden className="flex size-10 shrink-0 items-center justify-center rounded-[10px] bg-brand/10 text-brand">
        <BotIcon className="size-5" />
      </span>
      <div className="min-w-0 flex-1">
        <h2 id="space-ready-title" className="text-[14px] font-semibold">
          Let an agent use this Space
        </h2>
        <p className="mt-0.5 text-xs text-muted-foreground">{rows === null ? "Looking for your agents…" : line}</p>
      </div>
      <Button data-space-ready-run="" disabled={rows === null} onClick={onRun}>
        {action}
      </Button>
      {onDismiss ? (
        <Button data-space-ready-dismiss="" size="icon-sm" variant="ghost" aria-label="Not now" onClick={onDismiss}>
          <XIcon className="size-3.5" />
        </Button>
      ) : null}
    </section>
  );
}
