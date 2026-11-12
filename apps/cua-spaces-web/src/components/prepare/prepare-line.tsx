// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { PrepareView } from "@/bridge";

/**
 * Warming happens silently: this shows nothing in any state but one. When preparing failed and will not clear
 * by itself, Home says so quietly with Try again, because the first agent must stay recoverable. The words are
 * the core's (`prepare.view`: "Preparing stopped: ...").
 */
export function PrepareLine({ view, onRetry }: { view: PrepareView | null; onRetry?: () => void }) {
  if (!view || view.phase !== "failed") return null;
  return (
    <p data-prepare-line="" data-prepare-phase="failed" role="status" className="flex items-center gap-2 px-2 text-[12px] text-muted-foreground">
      <span aria-hidden className="size-1.5 shrink-0 rounded-full bg-destructive" />
      <span className="min-w-0 truncate">{view.line ?? "Preparing stopped"}</span>
      {view.action && onRetry ? (
        <button
          type="button"
          onClick={onRetry}
          data-prepare-action={view.action.id}
          className="shrink-0 text-foreground underline-offset-2 outline-none hover:underline focus-visible:ring-2 focus-visible:ring-ring/60"
        >
          {view.action.label}
        </button>
      ) : null}
    </p>
  );
}
