// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useState } from "react";

import { useBridge, type ApprovalAction, type ApprovalState, type ApprovalView, type KeyvaultOverview, type KvCommand } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";

/**
 * Review a waiting request (the SwiftUI `ApprovalSheet`): nothing is ticked
 * until the user ticks it, Approve sends exactly the ticked rows, and the
 * host confirms with Touch ID. The rows, the gate and the labels are the
 * core's (`approval.*`).
 */
export function ApprovalDialog({
  overview,
  labels,
  requestId,
  busy,
  onApprove,
  onDeny,
  onClose,
}: {
  overview: KeyvaultOverview;
  labels: { cancel: string; deny: string; confirmNote: string };
  requestId: string;
  busy: boolean;
  onApprove: (requestId: string, items: string[] | null) => void;
  onDeny: (requestId: string) => void;
  onClose: () => void;
}) {
  const { core } = useBridge();
  const [state, setState] = useState<ApprovalState>(() => core.call<ApprovalState>("approval.open", { requestId }));
  const view = core.tryCall<ApprovalView>("approval.view", { overview, state });
  const send = (action: ApprovalAction) => setState(core.call<ApprovalState>("approval.reduce", { overview, state, action }));
  if (!view) return null;
  const approve = () => {
    const command = core.tryCall<KvCommand | null>("approval.approveCommand", { overview, state });
    if (command?.type === "approve") onApprove(command.requestId, command.items);
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogPopup>
        <div className="px-5 pt-5 pb-3" data-approval>
          <DialogTitle className="text-[15px] font-semibold">{view.title}</DialogTitle>
          <p className="mt-1 text-xs text-muted-foreground" title={view.claims.join("\n")}>
            {view.badge.text} · to {view.targets} · {view.wants}
          </p>
          <ul className="mt-3 max-h-64 divide-y overflow-y-auto rounded-lg border bg-card">
            {view.rows.map((row) => (
              <li key={row.key}>
                <label className="flex cursor-default items-center gap-3 px-4 py-2.5" data-approval-row={row.key}>
                  <Checkbox aria-label={`Approve ${row.title}`} checked={row.selected} onCheckedChange={() => send({ type: "toggle", key: row.key })} />
                  <span className="min-w-0 flex-1 truncate text-[13px]">{row.title}</span>
                  <span className="shrink-0 text-xs text-muted-foreground">{row.account}</span>
                </label>
              </li>
            ))}
          </ul>
          {view.blockedReason ? <p className="mt-2 text-xs text-muted-foreground">{view.blockedReason}</p> : null}
        </div>
        <div className="flex items-center gap-2 border-t bg-muted/60 px-5 py-3">
          <span className="flex-1 text-xs text-muted-foreground">{labels.confirmNote}</span>
          <Button variant="outline" onClick={onClose}>
            {labels.cancel}
          </Button>
          <Button variant="outline" disabled={busy} onClick={() => onDeny(requestId)}>
            {labels.deny}
          </Button>
          <Button disabled={!view.canApprove || busy} onClick={approve} data-approval-approve>
            {view.approveLabel}
          </Button>
        </div>
      </DialogPopup>
    </Dialog>
  );
}
