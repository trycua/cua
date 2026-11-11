// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEnvironmentTag } from "@/bridge";
import { Tooltip } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

/** "Local · Staging" beside the page title, so a Local or staging build is
 * never mistaken for the released app (the core's `environment.tag`). The
 * released app on production shows nothing. Staging has a faint warm tint,
 * production stays neutral, so the two are told apart at a glance. */
export function EnvironmentTag() {
  const tag = useEnvironmentTag();
  if (!tag) return null;
  return (
    <Tooltip content={tag.description}>
      <span
        role="note"
        tabIndex={0}
        aria-label={tag.description}
        data-environment-tag={tag.env}
        data-no-drag=""
        className={cn(
          "inline-flex h-5 shrink-0 items-center whitespace-nowrap rounded-lg px-1.5 text-2xs font-medium outline-none focus-visible:ring-2 focus-visible:ring-ring/60",
          tag.env === "staging" ? "bg-warning/20 text-foreground" : "bg-foreground/[0.06] text-muted-foreground",
        )}
      >
        {tag.label}
      </span>
    </Tooltip>
  );
}
