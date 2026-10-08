// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Select as SelectPrimitive } from "@base-ui/react/select";
import { CheckIcon, ChevronsUpDownIcon } from "lucide-react";
import { Fragment } from "react";

import { cn } from "@/lib/utils";

export interface MenuItem {
  value: string;
  label: string;
  /** Its tooltip: why it is greyed out, or what it runs. */
  detail?: string;
  disabled?: boolean;
  /** Items with another group get a separator before them. */
  group?: string;
}

interface MenuSelectProps {
  value: string;
  items: readonly MenuItem[];
  onValueChange: (value: string) => void;
  disabled?: boolean;
  className?: string;
  "aria-label": string;
  /** Marks the trigger and each item for tests (`data-<name>`, `data-<name>-option`). */
  dataName?: string;
}

/**
 * A menu of choices with separators between groups, greyed-out entries and
 * a tooltip each: New Space's Run on and Runtime menus.
 */
export function MenuSelect({ value, items, onValueChange, disabled, className, dataName, ...rest }: MenuSelectProps) {
  const chosen = items.find((i) => i.value === value);
  const data = (suffix = "") => (dataName ? { [`data-${dataName}${suffix}`]: "" } : {});
  return (
    <SelectPrimitive.Root
      items={items.map((i) => ({ value: i.value, label: i.label }))}
      value={value}
      disabled={disabled}
      onValueChange={(v) => {
        if (v !== null && v !== value) onValueChange(v as string);
      }}
    >
      <SelectPrimitive.Trigger
        aria-label={rest["aria-label"]}
        title={chosen?.detail || undefined}
        {...data()}
        className={cn(
          "inline-flex h-7 min-w-44 items-center justify-between gap-2 rounded-md border border-input bg-card px-2.5 text-[13px] shadow-xs outline-none focus-visible:ring-2 focus-visible:ring-ring/60 data-disabled:opacity-50 dark:bg-input/30",
          className,
        )}
      >
        <SelectPrimitive.Value className="truncate" />
        <SelectPrimitive.Icon>
          <ChevronsUpDownIcon className="size-3.5 text-muted-foreground" />
        </SelectPrimitive.Icon>
      </SelectPrimitive.Trigger>
      <SelectPrimitive.Portal>
        <SelectPrimitive.Positioner sideOffset={4} alignItemWithTrigger={false} className="z-[60] outline-none">
          <SelectPrimitive.Popup className="glass max-h-(--available-height) min-w-(--anchor-width) origin-(--transform-origin) overflow-y-auto rounded-lg border p-1 text-popover-foreground shadow-float outline-none transition-[opacity,scale] data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0">
            <SelectPrimitive.List>
              {items.map((item, i) => (
                <Fragment key={item.value}>
                  {i > 0 && item.group !== items[i - 1]!.group ? <SelectPrimitive.Separator className="mx-1.5 my-1 h-px bg-border" /> : null}
                  <SelectPrimitive.Item
                    value={item.value}
                    disabled={item.disabled}
                    title={item.detail || undefined}
                    {...data("-option")}
                    data-value={item.value}
                    className="grid cursor-default grid-cols-[1rem_1fr] items-center gap-x-1.5 rounded-md py-1 pr-3 pl-1.5 text-[13px] outline-none data-disabled:text-muted-foreground data-highlighted:bg-brand data-highlighted:text-white"
                  >
                    <SelectPrimitive.ItemIndicator className="col-start-1">
                      <CheckIcon className="size-3.5" />
                    </SelectPrimitive.ItemIndicator>
                    <SelectPrimitive.ItemText className="col-start-2" data-item-label="">{item.label}</SelectPrimitive.ItemText>
                  </SelectPrimitive.Item>
                </Fragment>
              ))}
            </SelectPrimitive.List>
          </SelectPrimitive.Popup>
        </SelectPrimitive.Positioner>
      </SelectPrimitive.Portal>
    </SelectPrimitive.Root>
  );
}
