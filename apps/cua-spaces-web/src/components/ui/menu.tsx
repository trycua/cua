// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Menu as MenuPrimitive } from "@base-ui/react/menu";
import type { ReactElement, ReactNode } from "react";

import { cn } from "@/lib/utils";

export interface MenuItem {
  label: string;
  onSelect: () => void;
  destructive?: boolean;
  disabled?: boolean;
}

/** A small action menu on a trigger (a row's "…"). */
export function Menu({ trigger, items, label }: { trigger: ReactElement; items: readonly MenuItem[]; label?: ReactNode }) {
  return (
    <MenuPrimitive.Root>
      <MenuPrimitive.Trigger render={trigger} />
      <MenuPrimitive.Portal>
        <MenuPrimitive.Positioner sideOffset={4} align="end" className="z-50 outline-none">
          <MenuPrimitive.Popup className="glass min-w-36 origin-(--transform-origin) rounded-lg border p-1 text-popover-foreground shadow-float outline-none transition-[opacity,scale] data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0">
            {label ? <div className="px-2 py-1 text-2xs text-muted-foreground">{label}</div> : null}
            {items.map((item) => (
              <MenuPrimitive.Item
                key={item.label}
                disabled={item.disabled}
                onClick={item.onSelect}
                className={cn(
                  "flex cursor-default items-center rounded-md px-2 py-1 text-[13px] outline-none data-disabled:opacity-50 data-highlighted:bg-brand data-highlighted:text-white",
                  item.destructive && "text-destructive",
                )}
              >
                {item.label}
              </MenuPrimitive.Item>
            ))}
          </MenuPrimitive.Popup>
        </MenuPrimitive.Positioner>
      </MenuPrimitive.Portal>
    </MenuPrimitive.Root>
  );
}
