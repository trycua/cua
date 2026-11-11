// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Popover as PopoverPrimitive } from "@base-ui/react/popover";
import type { ComponentProps, ReactElement, ReactNode, RefObject } from "react";

import { cn } from "@/lib/utils";

/**
 * A panel on a trigger that holds its own controls (a field, a list with
 * arrow keys), where a menu would take the keys itself: the model menu and
 * the repo field of the ask box. The same glass and shadow as the menus.
 */
export function Popover({
  trigger,
  children,
  open,
  onOpenChange,
  width = 340,
  initialFocus,
  align = "start",
  popupProps,
}: {
  trigger: ReactElement;
  children: ReactNode;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** Width in px. */
  width?: number;
  /** The element that takes focus when it opens. */
  initialFocus?: RefObject<HTMLElement | null>;
  align?: "start" | "center" | "end";
  popupProps?: Omit<ComponentProps<typeof PopoverPrimitive.Popup>, "className" | "children" | "initialFocus">;
}) {
  return (
    <PopoverPrimitive.Root {...(open === undefined ? {} : { open })} onOpenChange={(o) => onOpenChange?.(o)}>
      <PopoverPrimitive.Trigger render={trigger} />
      <PopoverPrimitive.Portal>
        <PopoverPrimitive.Positioner sideOffset={6} align={align} className="z-50 outline-none">
          <PopoverPrimitive.Popup
            {...popupProps}
            initialFocus={initialFocus}
            style={{ width }}
            className={cn(
              "glass max-w-[calc(100vw-2rem)] origin-(--transform-origin) rounded-lg border p-1.5 text-popover-foreground shadow-float outline-none transition-[opacity,scale] data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0",
            )}
          >
            {children}
          </PopoverPrimitive.Popup>
        </PopoverPrimitive.Positioner>
      </PopoverPrimitive.Portal>
    </PopoverPrimitive.Root>
  );
}

/** A short rule between a panel's groups. */
export function PopoverDivider() {
  return <div role="separator" className="my-1 h-px bg-border" />;
}
