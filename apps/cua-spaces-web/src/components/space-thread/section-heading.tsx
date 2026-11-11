// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { ReactNode } from "react";

import type { SectionId } from "@/lib/space-layout";
import { cn } from "@/lib/utils";
import { useSpaceLayoutStore } from "@/stores/space-layout";

/**
 * A section's heading as its disclosure: a quiet chevron and the title in one
 * button (`aria-expanded`) that folds the section's body shut. Remembered per
 * person (`stores/space-layout.ts`).
 */
export function SectionHeading({
  id,
  folded,
  bodyId,
  children,
  className,
  inRow,
}: {
  id: SectionId;
  folded: boolean;
  bodyId: string;
  children: ReactNode;
  className?: string;
  /** The heading shares a row with controls: the row takes the hover, the button fills what is left of it. */ inRow?: boolean;
}) {
  const toggle = useSpaceLayoutStore((s) => s.toggleSection);
  return (
    <h2 className={cn("text-[11px] font-semibold tracking-wide text-muted-foreground uppercase", inRow && "flex-1 self-stretch", className)}>
      <button
        type="button"
        data-section-toggle={id}
        aria-expanded={!folded}
        aria-controls={bodyId}
        onClick={() => toggle(id)}
        className={cn(
          // No icon: the whole row is the control, a quiet fill on hover and on keyboard focus, edge to edge.
          "flex w-full cursor-pointer items-center px-5 text-left uppercase outline-none hover:text-foreground focus-visible:bg-accent focus-visible:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 focus-visible:ring-inset",
          inRow ? "h-full" : "h-8 hover:bg-accent",
        )}
      >
        {children}
      </button>
    </h2>
  );
}
