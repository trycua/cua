// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useT } from "@/i18n";
import { cn } from "@/lib/utils";

/** What the agent behind the box is doing: ready with its session open, starting, or needing a reconnect. */
export type AgentReadiness = "ready" | "starting" | "reconnect" | "none";

/**
 * One quiet line under the box, only when the agent isn't ready: "Starting agent"
 * while it starts, "Reconnect needed" with the way to fix it. Ready says nothing.
 * No badge and no animation: only the dot's colour changes.
 */
export function AgentStatus({ state, onReconnect }: { state: AgentReadiness; onReconnect?: () => void }) {
  const t = useT();
  if (state === "none" || state === "ready") return null;
  return (
    <p data-agent-status={state} className="flex items-center gap-2 px-2 text-[12px] text-muted-foreground" role="status">
      <span className={cn("size-1.5 rounded-full", state === "reconnect" ? "bg-warning" : "bg-muted-foreground/40")} aria-hidden />
      {state === "reconnect" ? t("home.agent_status.reconnect_needed") : t("home.agent_status.starting")}
      {state === "reconnect" && onReconnect ? (
        <button
          type="button"
          onClick={onReconnect}
          className="text-foreground underline-offset-2 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring/60"
        >
          {t("home.agent_status.reconnect")}
        </button>
      ) : null}
    </p>
  );
}
