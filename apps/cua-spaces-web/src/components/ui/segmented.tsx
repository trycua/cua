// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ToggleGroup } from "@base-ui/react/toggle-group";
import { Toggle } from "@base-ui/react/toggle";
import type { ReactNode } from "react";

import { cn } from "@/lib/utils";

interface SegmentedProps<V extends string> {
  value: V;
  options: readonly { value: V; label: ReactNode }[];
  onValueChange: (value: V) => void;
  className?: string;
  disabled?: boolean;
  "aria-label"?: string;
}

export function Segmented<V extends string>({ value, options, onValueChange, className, disabled, ...rest }: SegmentedProps<V>) {
  return (
    <ToggleGroup
      aria-label={rest["aria-label"]}
      disabled={disabled}
      value={[value]}
      onValueChange={(next) => {
        const v = next[0];
        if (v) onValueChange(v as V);
      }}
      className={cn("inline-flex h-7 items-center gap-0.5 rounded-md bg-muted p-0.5 data-disabled:opacity-50", className)}
    >
      {options.map((o) => (
        <Toggle
          key={o.value}
          value={o.value}
          className="inline-flex h-6 items-center gap-1.5 whitespace-nowrap rounded-[5px] px-2.5 text-xs font-medium text-muted-foreground outline-none transition-colors hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60 data-pressed:bg-card data-pressed:text-foreground data-pressed:shadow-xs dark:data-pressed:bg-accent"
        >
          {o.label}
        </Toggle>
      ))}
    </ToggleGroup>
  );
}
