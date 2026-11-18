// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useRef, useState, type ReactNode } from "react";

import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { setThisMacConfirmed, thisMacConfirmed, thisMacConfirmText, type TaskWhere } from "@/lib/ask-first";
import { useT } from "@/i18n";
import { thisComputerText } from "@/lib/host-labels";

/**
 * Picking where a task runs. This Mac asks once per device ("Use this Mac"
 * or "Pick another"); the answer is kept until it is reset in Settings →
 * Advanced. Declining calls `onDeclined` (the picker reopens).
 */
export function useWherePick(agent: string): {
  choose: (where: TaskWhere, onChosen: (w: TaskWhere) => void, onDeclined?: () => void) => void;
  dialog: ReactNode;
} {
  const t = useT();
  const [asking, setAsking] = useState<{ onChosen: (w: TaskWhere) => void; onDeclined?: () => void } | null>(null);
  // The dialog closes before its confirm runs: a close is a "Pick another" only if no confirm follows.
  const confirmed = useRef(false);
  const text = thisMacConfirmText(agent);
  return {
    choose: (where, onChosen, onDeclined) => {
      if (where.kind !== "this-mac" || thisMacConfirmed()) onChosen(where);
      else setAsking({ onChosen, onDeclined });
    },
    dialog: (
      <ConfirmDialog
        open={asking !== null}
        onOpenChange={(open) => {
          if (open || !asking) return;
          const declined = asking.onDeclined;
          setAsking(null);
          setTimeout(() => {
            if (!confirmed.current) declined?.();
            confirmed.current = false;
          }, 0);
        }}
        title={text.title}
        description={text.description}
        confirmLabel={t("home.this_mac.use", { computer: thisComputerText() })}
        cancelLabel={t("home.this_mac.pick_another")}
        onConfirm={() => {
          const chosen = asking?.onChosen;
          confirmed.current = true;
          setThisMacConfirmed(true);
          setAsking(null);
          chosen?.({ kind: "this-mac" });
        }}
      />
    ),
  };
}
