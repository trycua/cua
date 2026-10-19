// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link } from "@tanstack/react-router";

import { CuaKey } from "@/components/cua-key";
import { Button } from "@/components/ui/button";
import type { Meter } from "@/lib/resources";
import { cn } from "@/lib/utils";

/** A thin bar, 0 to 1. Blue; amber when the disk runs low; red when it's out. */
export function Bar({ fill, tone, className }: { fill: number; tone?: Meter["tone"]; className?: string }) {
  return (
    <div className={cn("h-1 overflow-hidden rounded-full bg-foreground/10", className)}>
      <div
        data-bar-fill={Math.round(fill * 100)}
        className={cn("h-full rounded-full transition-[width] duration-500 ease-out", tone === "critical" ? "bg-destructive" : tone === "warning" ? "bg-warning" : "bg-brand-strong")}
        style={{ width: `${Math.max(fill > 0 ? 2 : 0, fill * 100)}%` }}
      />
    </div>
  );
}

/** "Processor   12 cores · 38% busy", with its bar under it. */
export function MeterRow({ meter }: { meter: Meter }) {
  return (
    <div data-meter={meter.label} className="min-w-0">
      <div className="flex items-baseline justify-between gap-3 text-xs">
        <span className="shrink-0 text-muted-foreground">{meter.label}</span>
        <span className="truncate font-medium tabular-nums">{meter.value}</span>
      </div>
      <Bar fill={meter.fill} tone={meter.tone} className="mt-1.5" />
    </div>
  );
}

/** Three short meters in a row, value over bar, for a Space's tile. */
export function CompactMeters({ meters, className }: { meters: Meter[]; className?: string }) {
  return (
    <div data-space-meters="" className={cn("grid grid-cols-3 gap-2.5", className)}>
      {meters.map((m) => (
        <div key={m.label} data-meter={m.label} title={`${m.label}: ${m.help ?? m.value}`} className="min-w-0">
          <div className="truncate text-2xs text-muted-foreground tabular-nums">{m.value}</div>
          <Bar fill={m.fill} tone={m.tone} className="mt-1" />
        </div>
      ))}
    </div>
  );
}

/**
 * "This Mac is almost out of space. Delete old Spaces or snapshots.", with
 * the Cua key and "See what's using space" (the Spaces list, biggest
 * first). `compact` drops the second line, for inside a panel.
 */
export function LowDiskWarning({ warning, compact = false, className }: { warning: { level: "low" | "critical"; title: string; detail: string }; compact?: boolean; className?: string }) {
  return (
    <div
      role="status"
      data-low-disk={warning.level}
      className={cn(
        "flex items-center gap-3 px-4 py-3",
        warning.level === "critical" ? "bg-destructive/10" : "bg-warning/15",
        className,
      )}
    >
      <CuaKey size={24} />
      <div className="min-w-0 flex-1">
        <p className="text-[13px] font-semibold">{warning.title}</p>
        {compact ? null : <p className="mt-0.5 text-xs text-muted-foreground">{warning.detail}</p>}
      </div>
      <Button size="sm" variant="outline" data-see-space-use="" render={<Link to="/spaces" search={{ sort: "size" }} />}>
        See what’s using space
      </Button>
    </div>
  );
}
