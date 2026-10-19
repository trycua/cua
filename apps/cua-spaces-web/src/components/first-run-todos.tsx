// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useNavigate } from "@tanstack/react-router";
import { CheckIcon, XIcon } from "lucide-react";
import { useEffect, useState } from "react";

import { firstRunTodos, useBridge, useFirstRun, useFirstRunSaved, writeFirstRun } from "@/bridge";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** Whether Home shows the first run's to-do list (after Finish later). */
export function useFirstRunTodosShown(): boolean {
  const saved = useFirstRunSaved();
  return saved.finishedLater && !saved.todosClosed;
}

/**
 * "Finish setting up" on Home after the first run's Finish later: the same
 * checks, kept, each with its fix (the core's `firstRun.todos`). It goes
 * away once everything required is done, or when closed.
 */
export function FirstRunTodosCard() {
  const shown = useFirstRunTodosShown();
  return shown ? <TodosCard /> : null;
}

function TodosCard() {
  const fr = useFirstRun(true);
  const { core } = useBridge();
  const todos = firstRunTodos(core, { ...fr.input, step: "setup" });
  const [copied, setCopied] = useState(false);
  const navigate = useNavigate();
  useEffect(() => {
    if (todos?.complete) writeFirstRun({ todosClosed: true });
  }, [todos?.complete]);
  if (!todos) return null;
  return (
    <section className="mb-6 overflow-hidden rounded-xl border bg-card" aria-labelledby="first-run-todos" data-first-run-todos="">
      <header className="flex items-start justify-between gap-4 border-b px-5 py-3.5">
        <div>
          <h2 id="first-run-todos" className="text-[14px] font-semibold">
            {todos.title}
          </h2>
          <p className="text-[13px] text-muted-foreground">{todos.lede}</p>
        </div>
        <div className="flex items-center gap-2">
          <span className="text-[13px] text-muted-foreground">{todos.progress}</span>
          <Button variant="ghost" size="icon-sm" aria-label="Close" onClick={() => writeFirstRun({ todosClosed: true })}>
            <XIcon className="size-3.5" />
          </Button>
        </div>
      </header>
      <ul className="divide-y">
        {todos.items.map((t) => (
          <li key={t.id} className="flex items-center gap-3.5 px-5 py-2.5" data-first-run-todo={t.id} data-done={t.done ? "" : undefined}>
            <span
              aria-hidden
              className={cn(
                "flex size-5 shrink-0 items-center justify-center rounded-full border",
                t.done ? "border-brand bg-brand text-white" : "border-foreground/25",
              )}
            >
              {t.done ? <CheckIcon className="size-3" strokeWidth={3} /> : null}
            </span>
            <div className="min-w-0 flex-1">
              <p className={cn("text-[13px] font-medium", t.done && "text-muted-foreground")}>
                {t.title}
                <span className="sr-only">{t.done ? " (done)" : ""}</span>
              </p>
              <p className="truncate text-xs text-muted-foreground">{t.note}</p>
            </div>
            {t.action ? (
              <Button
                variant="outline"
                size="sm"
                className="shrink-0"
                onClick={() => {
                  if (t.action!.id === "copy") {
                    void fr.copyPrompt().then(() => {
                      setCopied(true);
                      setTimeout(() => setCopied(false), 1500);
                    });
                  } else if (t.action!.id === "add") {
                    // Back to the first run's choice of system (macOS or Linux).
                    void fr.extra("add").then(() => navigate({ to: "/onboarding" }));
                  } else {
                    void fr.act(t.action!.id, t.settingsUrl);
                  }
                }}
              >
                {t.action.id === "copy" && copied ? "Copied" : t.action.label}
              </Button>
            ) : null}
          </li>
        ))}
      </ul>
    </section>
  );
}
