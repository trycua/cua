// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { AgentRunLine } from "@/bridge";

/**
 * A Space's Agents section, as the SwiftUI detail's `AgentRows`: one line
 * per run (the agent, then what it was asked) with its status on the right
 * and the reason as the tooltip. "Could not read" is its own line, never
 * an empty list.
 */
export function AgentsSection({
  title,
  runs,
  failed,
  copy,
}: {
  title: string;
  runs: AgentRunLine[] | null;
  failed: string | null;
  copy: { loading: string; empty: string; failed: string };
}) {
  const note = (text: string, hint?: string) => (
    <li data-agents-status="" title={hint} className="px-4 py-2.5 text-[13px] text-muted-foreground">
      {text}
    </li>
  );
  return (
    <section data-section="agents" className="mb-7">
      <h2 className="mb-2 px-1 text-xs font-semibold text-muted-foreground">{title}</h2>
      <ul className="divide-y overflow-hidden rounded-xl border bg-card shadow-xs">
        {failed
          ? note(copy.failed, failed)
          : runs === null
            ? note(copy.loading)
            : runs.length === 0
              ? note(copy.empty)
              : runs.map((r) => (
                  <li key={r.runId} data-agent-run={r.runId} title={r.reason} className="flex min-h-10 items-center justify-between gap-4 px-4 py-2.5 text-[13px]">
                    <span className="min-w-0 truncate">{r.line}</span>
                    <span className="shrink-0 text-muted-foreground">{r.status}</span>
                  </li>
                ))}
      </ul>
    </section>
  );
}
