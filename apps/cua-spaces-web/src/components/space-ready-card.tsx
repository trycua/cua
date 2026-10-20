// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { BotIcon } from "lucide-react";

import type { Space } from "@/bridge";
import { Button } from "@/components/ui/button";

/**
 * The moment a user's first Space is ready: one thing to do next, "Run an
 * agent here". Add computer and Share stay in the checklist below it.
 */
export function SpaceReadyCard({ space, onRun }: { space: Space; onRun: () => void }) {
  return (
    <section data-space-ready="" aria-labelledby="space-ready-title" className="mb-6 flex items-center gap-4 rounded-xl border bg-card px-5 py-4 shadow-xs">
      <span aria-hidden className="flex size-10 shrink-0 items-center justify-center rounded-[10px] bg-brand/10 text-brand">
        <BotIcon className="size-5" />
      </span>
      <div className="min-w-0 flex-1">
        <h2 id="space-ready-title" className="text-[14px] font-semibold">
          {space.name} is ready
        </h2>
        <p className="mt-0.5 text-xs text-muted-foreground">Give an agent a task and watch it work on this computer.</p>
      </div>
      <Button data-space-ready-run="" onClick={onRun}>
        Run an agent here
      </Button>
    </section>
  );
}
