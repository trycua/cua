// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Menu as MenuPrimitive } from "@base-ui/react/menu";
import { CheckIcon } from "lucide-react";
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
                  "flex cursor-default items-center rounded-lg px-2 py-1 text-[13px] outline-none data-disabled:opacity-50 data-highlighted:bg-brand data-highlighted:text-white",
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

export interface ChoiceItem {
  key: string;
  /** What the row shows (an icon and two lines). */
  content: ReactNode;
  selected?: boolean;
  onSelect: () => void;
  /** `data-*` attributes for the row. */
  attrs?: Record<`data-${string}`, string | undefined>;
}

/**
 * A pop-up menu to pick one of several things, each a rich row with a check on the one in use, and an
 * action row below a rule ("Connect another provider"). The trigger is a button.
 */
export function ChoiceMenu({
  trigger,
  items,
  action,
  popupAttrs,
}: {
  trigger: ReactElement;
  items: readonly ChoiceItem[];
  action?: { content: ReactNode; onSelect: () => void; attrs?: Record<`data-${string}`, string | undefined> };
  popupAttrs?: Record<`data-${string}`, string | undefined>;
}) {
  const row = "flex cursor-default items-center gap-2.5 rounded-lg px-2 py-1.5 text-[13px] outline-none data-highlighted:bg-accent";
  return (
    <MenuPrimitive.Root>
      <MenuPrimitive.Trigger render={trigger} />
      <MenuPrimitive.Portal>
        <MenuPrimitive.Positioner sideOffset={4} align="end" className="z-50 outline-none">
          <MenuPrimitive.Popup
            {...popupAttrs}
            className="glass min-w-64 max-w-[min(360px,calc(100vw-2rem))] origin-(--transform-origin) rounded-lg border p-1 text-popover-foreground shadow-float outline-none transition-[opacity,scale] data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0"
          >
            {items.map((item) => (
              <MenuPrimitive.Item key={item.key} onClick={item.onSelect} data-selected={item.selected ? "" : undefined} {...item.attrs} className={row}>
                <span className="flex min-w-0 flex-1 items-center gap-2.5">{item.content}</span>
                {item.selected ? <CheckIcon className="size-3.5 shrink-0 text-foreground" /> : null}
              </MenuPrimitive.Item>
            ))}
            {action ? (
              <>
                <MenuPrimitive.Separator className="my-1 h-px bg-border" />
                <MenuPrimitive.Item onClick={action.onSelect} {...action.attrs} className={row}>
                  {action.content}
                </MenuPrimitive.Item>
              </>
            ) : null}
          </MenuPrimitive.Popup>
        </MenuPrimitive.Positioner>
      </MenuPrimitive.Portal>
    </MenuPrimitive.Root>
  );
}
