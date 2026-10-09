// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Checkbox } from "@/components/ui/checkbox";
import { DriverCard, DriverSummaries } from "./driver-card";
import type { AgentsStep } from "./use-agents-step";

/** The page's card: the agents, what goes in, the cua-driver card; then what was set up. */
export function AgentsCard({ step }: { step: AgentsStep }) {
  const { copy } = step;
  if (step.summaries) {
    return (
      <div className="flex flex-col gap-3">
        <h2 className="text-[13px] font-semibold">{copy.agentsDoneTitle}</h2>
        <DriverSummaries summaries={step.summaries} />
        {step.error ? <p className="text-xs text-muted-foreground">{step.error}</p> : null}
      </div>
    );
  }
  if (step.agents === null) return <p className="text-[13px] text-muted-foreground">{copy.agentsLooking}</p>;
  if (step.agents.length === 0) return <p className="text-[13px] text-muted-foreground">{copy.agentsNone}</p>;
  return (
    <div className="flex flex-col gap-3">
      <div className="grid max-h-[132px] grid-cols-2 gap-x-4 gap-y-1.5 overflow-y-auto" data-agents-list="">
        {step.agents.map((a) => (
          <label key={a.agent} className="flex min-w-0 items-center gap-2 text-[13px]" title={a.mcpConfig ?? a.skillsDir ?? undefined}>
            <Checkbox checked={step.selected.has(a.agent)} disabled={step.busy} onCheckedChange={(on) => step.toggle(a.agent, Boolean(on))} />
            <span className="truncate">{a.name}</span>
          </label>
        ))}
      </div>
      <div className="border-t" />
      <label className="flex items-center gap-2 text-[13px]">
        <Checkbox checked={step.skills} disabled={step.busy} onCheckedChange={(on) => step.setSkills(Boolean(on))} />
        {copy.agentsSkills} and {copy.agentsMcp}
      </label>
      <DriverCard checked={step.driver} onCheckedChange={step.setDriver} disabled={step.busy} />
      {step.error ? (
        <p className="text-xs text-destructive" role="alert">
          {step.error}
        </p>
      ) : null}
    </div>
  );
}
