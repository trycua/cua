// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { CiAutopilotView, CiGoal } from "@/bridge";
import { GroupedRow, GroupedSection } from "@/components/ui/grouped";
import { Segmented } from "@/components/ui/segmented";
import { cn } from "@/lib/utils";

/**
 * A cloud's automatic sizing: the cloud, the goal (cheapest, balanced, fastest) and what it
 * means, the budget used this month, what it decided for each kind of job and
 * the reasons, in the sizing's own sentences.
 */
export function Autopilot({
  pilot,
  heading,
  cloudLabel,
  onGoal,
}: {
  pilot: CiAutopilotView;
  heading: string;
  cloudLabel: string;
  onGoal(target: string, goal: CiGoal): void;
}) {
  return (
    <GroupedSection title={heading} data-ci-autopilot={pilot.target}>
      <GroupedRow className="justify-between gap-6">
        <span className="text-[13px]">{cloudLabel}</span>
        <span className="text-[13px] text-muted-foreground" data-ci-cloud="">
          {pilot.title}
        </span>
      </GroupedRow>
      <GroupedRow className="items-start justify-between gap-6">
        <div className="min-w-0">
          <p className="text-[13px]">Goal</p>
          <p className="mt-px text-xs text-muted-foreground" data-ci-goal-note="">
            {pilot.goalNote}
          </p>
        </div>
        <Segmented<CiGoal>
          aria-label="Goal"
          value={pilot.goals.find((g) => g.selected)?.id ?? "balanced"}
          options={pilot.goals.map((g) => ({ value: g.id, label: g.label }))}
          onValueChange={(goal) => onGoal(pilot.target, goal)}
        />
      </GroupedRow>
      <GroupedRow className="justify-between gap-6">
        <span className="text-[13px]">Budget</span>
        <span
          className={cn(
            "text-[13px] tabular-nums text-muted-foreground",
            pilot.budgetTone === "warn" && "text-warning",
            pilot.budgetTone === "bad" && "text-destructive",
          )}
          data-ci-budget=""
        >
          {pilot.budget}
        </span>
      </GroupedRow>
      {pilot.plans.length > 0 ? (
        <div className="space-y-1.5 px-4 py-3" data-ci-plans="">
          {pilot.plans.map((p) => (
            <p key={p.label} className="text-[13px]">
              <span className="font-medium">{p.label}</span> <span className="text-muted-foreground">{p.line}</span>
            </p>
          ))}
        </div>
      ) : null}
      {pilot.reasons.length > 0 ? (
        <ul className="space-y-1.5 px-4 py-3 text-xs text-muted-foreground" data-ci-reasons="">
          {pilot.reasons.map((r) => (
            <li key={r}>{r}</li>
          ))}
        </ul>
      ) : null}
    </GroupedSection>
  );
}
