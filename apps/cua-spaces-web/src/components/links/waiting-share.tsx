// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useState } from "react";

import type { ShareLink } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { toastError } from "@/components/ui/toast";

/**
 * A public TCP share that is not live yet and waits for the person: the first
 * share through a new share edge (its cost, in the daemon's words) or an agent
 * in a Space asking to share a port. The words are the daemon's.
 */
export function WaitingShare({
  link,
  onApprove,
  onDeny,
}: {
  link: ShareLink;
  onApprove: (always: boolean) => Promise<unknown>;
  onDeny: () => Promise<unknown>;
}) {
  const [busy, setBusy] = useState(false);
  const [always, setAlways] = useState(false);
  const consent = link.waiting === "consent";
  const run = (f: () => Promise<unknown>, failed: string) => {
    setBusy(true);
    f()
      .catch(toastError(failed))
      .finally(() => setBusy(false));
  };
  return (
    <div className="flex flex-col gap-2.5" data-waiting-share={link.waiting ?? ""}>
      <p className="text-[13px]" data-waiting-text="">
        {link.waitingText}
      </p>
      {!consent ? (
        <label className="flex items-center gap-2 text-xs text-muted-foreground">
          <Checkbox checked={always} onCheckedChange={(v) => setAlways(v === true)} disabled={busy} data-waiting-always="" />
          Always allow this Space
        </label>
      ) : null}
      <div className="flex justify-end gap-2">
        <Button
          size="sm"
          variant="outline"
          disabled={busy}
          onClick={() => run(onDeny, consent ? "Could not cancel the share" : "Could not turn the share down")}
          data-waiting-deny=""
        >
          {consent ? "Cancel" : "Turn down"}
        </Button>
        <Button
          size="sm"
          disabled={busy}
          onClick={() => run(() => onApprove(!consent && always), consent ? "Could not create the server" : "Could not approve the share")}
          data-waiting-approve=""
        >
          {consent ? "Create server" : "Approve"}
        </Button>
      </div>
    </div>
  );
}
