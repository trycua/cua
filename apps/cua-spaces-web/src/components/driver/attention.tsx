// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { DriverStatus } from "@/bridge";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

import { driverAttention, type DriverAttention } from "./copy";

const DOT: Record<DriverAttention["kind"], string> = {
  "needs-permission": "bg-warning",
  failed: "bg-destructive",
  stopped: "bg-destructive",
  update: "bg-brand",
};

/**
 * The dot and word beside Cua Driver when it needs a person (the state dot's
 * style, `state-dot.tsx`); nothing while it works.
 */
export function DriverAttentionLabel({ status, className }: { status: DriverStatus | null; className?: string }) {
  const attention = status ? driverAttention(status) : null;
  if (!attention) return null;
  return (
    <Tooltip content={attention.tip} side="right">
      <span data-driver-attention={attention.kind} className={cn("inline-flex min-w-0 items-center gap-1.5 text-2xs text-muted-foreground", className)}>
        <span className={cn("size-1.5 shrink-0 rounded-full", DOT[attention.kind])} aria-hidden />
        <span className="truncate">{attention.word}</span>
      </span>
    </Tooltip>
  );
}
