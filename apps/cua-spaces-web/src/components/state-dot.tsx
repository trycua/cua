// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useBridge, type Space } from "@/bridge";
import { STATE_LABEL, type SpaceState } from "@/lib/spaces";
import { cn } from "@/lib/utils";

const DOT: Record<SpaceState, string> = {
  failed: "bg-destructive",
  running: "bg-success",
  stopped: "bg-muted-foreground/40",
  creating: "bg-brand animate-pulse",
  deleting: "bg-destructive/70 animate-pulse",
};

/**
 * A Space's state: the dot, and the word the core gives its row
 * (`sidebar.statusText`, as the SwiftUI sidebar and detail say it:
 * "Suspended", "Stopped", "Turning on…") when `space` is given, else this
 * page's word for the state.
 */
export function StateLabel({ state, space, className }: { state: SpaceState; space?: Space; className?: string }) {
  const { core } = useBridge();
  const word = space ? (core.tryCall<string>("sidebar.statusText", { space }) ?? STATE_LABEL[state]) : STATE_LABEL[state];
  return (
    <span data-state-label={state} className={cn("inline-flex items-center gap-1.5 text-xs text-muted-foreground", className)}>
      <span className={cn("size-1.5 rounded-full", DOT[state])} aria-hidden />
      {word}
    </span>
  );
}
