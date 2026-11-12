// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CircleCheckIcon, CircleDashedIcon, CircleIcon } from "lucide-react";

import type { CiSetupRow, CiSetupView } from "@/bridge";
import { Button } from "@/components/ui/button";
import { GroupedRow, GroupedSection } from "@/components/ui/grouped";
import { cn } from "@/lib/utils";

const ICON = { done: CircleCheckIcon, todo: CircleIcon, pending: CircleDashedIcon } as const;

/**
 * The setup checklist: connect GitHub, give access to repositories, use the
 * runner label in a workflow, a job passed. The words are the core's
 * (`ciRunners.view`); it is shown until all four are done. Each undone step
 * says what to do and offers the button that does it.
 */
export function SetupChecklist({ setup, onAction }: { setup: CiSetupView; onAction(id: string): void }) {
  return (
    <GroupedSection
      title={setup.title}
      action={
        <span className="text-xs text-muted-foreground" data-ci-progress="">
          {setup.progress}
        </span>
      }
      data-ci-setup=""
    >
      {setup.rows.map((row) => (
        <SetupRow key={row.id} row={row} onAction={onAction} />
      ))}
    </GroupedSection>
  );
}

function SetupRow({ row, onAction }: { row: CiSetupRow; onAction(id: string): void }) {
  const Icon = ICON[row.state];
  return (
    <GroupedRow data-ci-setup-row={row.id} data-state={row.state} className="gap-3">
      <Icon className={cn("size-4 shrink-0", row.state === "done" ? "text-success" : "text-muted-foreground")} aria-hidden />
      <div className="min-w-0 flex-1">
        <p className={cn("text-[13px]", row.state === "done" && "text-muted-foreground")}>{row.title}</p>
        {row.detail ? <p className="mt-px text-xs text-muted-foreground">{row.detail}</p> : null}
      </div>
      {row.action ? (
        <Button size="sm" variant="outline" data-ci-action={row.action.id} onClick={() => onAction(row.action!.id)}>
          {row.action.label}
        </Button>
      ) : null}
    </GroupedRow>
  );
}
