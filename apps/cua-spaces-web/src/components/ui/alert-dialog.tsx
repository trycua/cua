// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { AlertDialog as AlertDialogPrimitive } from "@base-ui/react/alert-dialog";
import type { ReactNode } from "react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

export const AlertDialog = AlertDialogPrimitive.Root;

export function AlertDialogPopup({ className, children, ...props }: AlertDialogPrimitive.Popup.Props) {
  return (
    <AlertDialogPrimitive.Portal>
      <AlertDialogPrimitive.Backdrop data-window-dim="" className="fixed inset-0 z-50 bg-black/25 backdrop-blur-[2px] transition-opacity duration-150 data-ending-style:opacity-0 data-starting-style:opacity-0 dark:bg-black/40" />
      <AlertDialogPrimitive.Popup
        className={cn(
          "glass fixed top-[22vh] left-1/2 z-50 w-[min(420px,calc(100vw-2rem))] -translate-x-1/2 overflow-hidden rounded-xl border text-popover-foreground shadow-float outline-none transition-[opacity,scale] duration-150 ease-out-soft data-ending-style:scale-[0.98] data-ending-style:opacity-0 data-starting-style:scale-[0.98] data-starting-style:opacity-0",
          className,
        )}
        {...props}
      >
        {children}
      </AlertDialogPrimitive.Popup>
    </AlertDialogPrimitive.Portal>
  );
}

interface ConfirmDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: ReactNode;
  confirmLabel: string;
  /** Red confirm button, for actions that lose work or data. */
  destructive?: boolean;
  /** The other button (default "Cancel"). */
  cancelLabel?: string;
  /** Why the confirm button can't be pressed; it shows disabled with this beside it. */
  disabledReason?: string | null;
  onConfirm: () => void;
  /** A second way through, beside the confirm button ("Remove from List"). */
  alternative?: { label: string; onClick: () => void } | null;
}

/** A yes-or-no question before an action that is hard to undo. */
export function ConfirmDialog({ open, onOpenChange, title, description, confirmLabel, destructive, cancelLabel = "Cancel", disabledReason, onConfirm, alternative }: ConfirmDialogProps) {
  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogPopup>
        <div className="px-5 pt-5 pb-4">
          <AlertDialogPrimitive.Title className="text-[15px] font-semibold">{title}</AlertDialogPrimitive.Title>
          <AlertDialogPrimitive.Description className="mt-1.5 text-[13px] text-muted-foreground">{description}</AlertDialogPrimitive.Description>
          {disabledReason ? <p className="mt-2 text-xs text-muted-foreground">{disabledReason}</p> : null}
        </div>
        <div className="flex justify-end gap-2 border-t bg-muted/60 px-5 py-3">
          <AlertDialogPrimitive.Close render={<Button variant="outline" />}>{cancelLabel}</AlertDialogPrimitive.Close>
          {alternative ? (
            <Button
              variant="outline"
              onClick={() => {
                onOpenChange(false);
                alternative.onClick();
              }}
            >
              {alternative.label}
            </Button>
          ) : null}
          <Button
            variant={destructive ? "destructive" : "default"}
            disabled={Boolean(disabledReason)}
            onClick={() => {
              onOpenChange(false);
              onConfirm();
            }}
          >
            {confirmLabel}
          </Button>
        </div>
      </AlertDialogPopup>
    </AlertDialog>
  );
}
