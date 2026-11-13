// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { CiStat } from "@/bridge";
import { cn } from "@/lib/utils";

/** The overview line: starts in the last 24 hours, jobs, pass rate, spend and the month's budget. Plain text in a row, no tiles. */
export function StatsLine({ stats }: { stats: CiStat[] }) {
  return (
    <dl data-ci-stats="" className="grid grid-cols-2 gap-x-6 gap-y-3 px-1 sm:grid-cols-3 lg:grid-cols-5">
      {stats.map((s) => (
        <div key={s.label} data-ci-stat={s.label} className="min-w-0">
          <dt className="truncate text-xs text-muted-foreground">{s.label}</dt>
          <dd
            className={cn(
              "mt-0.5 text-[17px] font-semibold tracking-[-0.01em] tabular-nums",
              s.tone === "warn" && "text-warning",
              s.tone === "bad" && "text-destructive",
            )}
          >
            {s.value}
          </dd>
        </div>
      ))}
    </dl>
  );
}
