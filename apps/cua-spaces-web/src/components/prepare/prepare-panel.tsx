// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, LoaderCircleIcon } from "lucide-react";

import type { PrepareStepState, PrepareView } from "@/bridge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** One mark per step state: a check when done, a turning ring while it runs, a quiet dot while it waits. */
function Mark({ state }: { state: PrepareStepState }) {
  if (state === "done" || state === "skipped") return <CheckIcon aria-hidden className="size-3.5 text-success" />;
  if (state === "running") return <LoaderCircleIcon aria-hidden className="size-3.5 animate-spin text-muted-foreground" />;
  if (state === "failed") return <span aria-hidden className="size-1.5 rounded-full bg-destructive" />;
  if (state === "waiting") return <span aria-hidden className="size-1.5 rounded-full bg-warning" />;
  return <span aria-hidden className="size-1.5 rounded-full bg-foreground/20" />;
}

/**
 * Where the first Space is, as one quiet block like the sidebar's "Making"
 * item: four plain lines (Linux runtime, Agent image, Home Space, the coding
 * agent), each a small mark, its label and muted detail, and one thin bar
 * under the current step only. No boxes, no borders, no rows between lines.
 * Every word is the core's (`prepare.view`); this only draws it.
 */
export function PreparePanel({ view, onRetry }: { view: PrepareView; onRetry?: () => void }) {
  return (
    <div data-prepare={view.phase} role="group" aria-label={view.title} className="flex w-full max-w-[420px] flex-col px-6 text-left">
      {view.line ? (
        <p data-prepare-line="" role="status" className="mb-3 text-center text-[13px] text-muted-foreground">
          {view.line}
        </p>
      ) : null}
      <ol data-prepare-steps="" className="flex flex-col gap-2.5">
        {view.steps.map((s) => (
          <li key={s.id} data-prepare-step={s.id} data-state={s.state}>
            <div className="flex items-center gap-2.5 text-[13px]">
              <span className="flex w-4 shrink-0 items-center justify-center">
                <Mark state={s.state} />
              </span>
              <span className={cn("min-w-0 flex-1 whitespace-nowrap", s.state === "pending" ? "text-muted-foreground" : "")}>{s.label}</span>
              {s.detail ? <span className="shrink-0 text-xs text-muted-foreground tabular-nums">{s.detail}</span> : null}
            </div>
            {s.fraction != null ? (
              <div data-prepare-bar="" className="mt-1.5 ml-[26px] h-1 overflow-hidden rounded-full bg-foreground/10" aria-hidden>
                <div className="h-full rounded-full bg-brand transition-[width]" style={{ width: `${Math.round(s.fraction * 100)}%` }} />
              </div>
            ) : null}
          </li>
        ))}
      </ol>
      {view.composerHint ? (
        <p data-prepare-hint="" className="mt-4 text-center text-xs text-muted-foreground">
          {view.composerHint}
        </p>
      ) : null}
      {view.action && onRetry ? (
        <Button className="mx-auto mt-3" variant="outline" onClick={onRetry} data-prepare-action={view.action.id}>
          {view.action.label}
        </Button>
      ) : null}
    </div>
  );
}
