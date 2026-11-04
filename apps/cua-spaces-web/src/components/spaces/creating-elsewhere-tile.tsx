// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A Space another of your computers is creating (CUA-2349), on the Spaces
 * page's grid: drawn like a Space being created here, with that computer's
 * own "Creating macOS on Mac mini…" and how far it got. Nothing to open yet
 * (not a link); once it is ready it is a Space like any other.
 */

import type { CreatingElsewhere } from "@/bridge";
import { OsIcon } from "@/components/os-icon";
import { creatingElsewhereLine, creatingElsewherePercent, creatingElsewherePlace, elsewhereOs } from "@/lib/spaces";

export function CreatingElsewhereTile({ creating }: { creating: CreatingElsewhere }) {
  const os = elsewhereOs(creating);
  const line = creatingElsewhereLine(creating);
  return (
    <div data-creating-elsewhere={creating.machineId} data-state="creating" role="group" aria-label={`${creating.name}: ${line}`}>
      <div className="relative flex aspect-[16/10] items-center justify-center overflow-hidden rounded-lg bg-tile">
        <OsIcon os={os} className="size-6 text-muted-foreground/25" />
        <div className="absolute inset-x-3 bottom-8">
          <div className="h-1 overflow-hidden rounded-full bg-foreground/10">
            <div data-create-progress className="h-full rounded-full bg-brand transition-[width] duration-500" style={{ width: `${Math.min(100, Math.max(0, creating.percent))}%` }} />
          </div>
        </div>
      </div>
      <div className="flex gap-3 pt-3">
        <OsIcon os={os} className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <div className="min-w-0 flex-1">
          <div className="flex min-w-0 items-baseline gap-3">
            <p data-space-name="" className="min-w-0 flex-1 truncate text-[13px] leading-5 font-semibold">
              {creating.name}
            </p>
            {/* How far, where this computer's own creates say "Creating". */}
            <span data-state-label="creating" className="inline-flex shrink-0 items-center gap-1.5 text-xs tabular-nums text-muted-foreground">
              <span className="size-1.5 animate-pulse rounded-full bg-brand" aria-hidden />
              {creatingElsewherePercent(creating)}
            </span>
          </div>
          <p className="truncate text-xs text-muted-foreground" title={line}>
            {creatingElsewherePlace(creating)}
          </p>
        </div>
      </div>
    </div>
  );
}
