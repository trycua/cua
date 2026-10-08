// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Select as SelectPrimitive } from "@base-ui/react/select";
import { CheckIcon, ChevronsUpDownIcon } from "lucide-react";

import { cn } from "@/lib/utils";

export interface SelectOption<V extends string | number> {
  value: V;
  label: string;
}

interface SelectProps<V extends string | number> {
  value: V;
  options: readonly SelectOption<V>[];
  onValueChange: (value: V) => void;
  className?: string;
  disabled?: boolean;
  "aria-label"?: string;
}

export function Select<V extends string | number>({ value, options, onValueChange, className, disabled, ...rest }: SelectProps<V>) {
  return (
    <SelectPrimitive.Root
      items={options}
      value={value}
      disabled={disabled}
      onValueChange={(v) => {
        if (v !== null) onValueChange(v as V);
      }}
    >
      <SelectPrimitive.Trigger
        aria-label={rest["aria-label"]}
        className={cn(
          "inline-flex h-7 min-w-36 items-center justify-between gap-2 rounded-md border border-input bg-card px-2.5 text-[13px] shadow-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/60 data-disabled:opacity-50 dark:bg-input/30",
          className,
        )}
      >
        <SelectPrimitive.Value />
        <SelectPrimitive.Icon>
          <ChevronsUpDownIcon className="size-3.5 text-muted-foreground" />
        </SelectPrimitive.Icon>
      </SelectPrimitive.Trigger>
      <SelectPrimitive.Portal>
        <SelectPrimitive.Positioner sideOffset={4} className="z-50 outline-none">
          <SelectPrimitive.Popup className="glass min-w-(--anchor-width) origin-(--transform-origin) rounded-lg border p-1 text-popover-foreground shadow-float outline-none transition-[opacity,scale] data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0">
            <SelectPrimitive.List>
              {options.map((o) => (
                <SelectPrimitive.Item
                  key={String(o.value)}
                  value={o.value}
                  className="grid cursor-default grid-cols-[1rem_1fr] items-center gap-1.5 rounded-md py-1 pr-3 pl-1.5 text-[13px] outline-none data-highlighted:bg-brand data-highlighted:text-white"
                >
                  <SelectPrimitive.ItemIndicator className="col-start-1">
                    <CheckIcon className="size-3.5" />
                  </SelectPrimitive.ItemIndicator>
                  <SelectPrimitive.ItemText className="col-start-2">{o.label}</SelectPrimitive.ItemText>
                </SelectPrimitive.Item>
              ))}
            </SelectPrimitive.List>
          </SelectPrimitive.Popup>
        </SelectPrimitive.Positioner>
      </SelectPrimitive.Portal>
    </SelectPrimitive.Root>
  );
}
