// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { CheckIcon, LoaderCircleIcon } from "lucide-react";
import { useEffect, useState, type FormEvent } from "react";

import { displayPairingCode, normalizePairingCode, pairingLinkRelay, useJoinWithCode, useMachines } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogDescription, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";

/**
 * "Join with a code": adds this computer to the account that made a
 * one-time code in "Add a machine" on another device. Meant for a computer
 * that has Cua Spaces but is not signed in to that account (a team machine,
 * or a computer you don't want to sign in on). `initialCode` is a code that
 * arrived by a `cua://add/<code>` link; it fills the field, and nothing
 * joins until the person presses Join.
 */
export function JoinWithCodeDialog({
  open,
  onOpenChange,
  initialCode = "",
  initialRelay = null,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  initialCode?: string;
  initialRelay?: string | null;
}) {
  const { refresh } = useMachines();
  const joining = useJoinWithCode({ onJoined: () => void refresh().catch(() => {}) });
  const [text, setText] = useState(initialCode);
  const { reset } = joining;
  useEffect(() => {
    if (open) {
      setText(initialCode ? displayPairingCode(initialCode) : "");
      reset();
    }
  }, [open, initialCode, reset]);

  const busy = joining.phase === "joining";
  const joined = joining.phase === "joined";
  const ready = normalizePairingCode(text) !== null;
  const relay = initialRelay ?? pairingLinkRelay(text);
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!busy && !joined && ready) void joining.join(text, relay);
  };

  return (
    <Dialog open={open} onOpenChange={(next) => (busy ? undefined : onOpenChange(next))}>
      <DialogPopup className="top-[14vh] w-[min(460px,calc(100vw-2rem))]" data-join-with-code={joining.phase}>
        <form onSubmit={submit} className="px-6 pt-5 pb-4">
          <DialogTitle className="text-[15px] font-semibold">Join with a code</DialogTitle>
          <DialogDescription className="mt-1 text-[13px] text-muted-foreground">
            Paste the code from Add a machine on your other device. This computer is added to the account that made it, and that account can then reach it. Only use a code you made yourself or got from someone you trust.
          </DialogDescription>
          {relay ? (
            <p className="mt-2 text-xs text-muted-foreground" data-join-relay>
              This code uses its own relay: <span className="font-medium text-foreground">{relay}</span>
            </p>
          ) : null}
          {joined ? (
            <div className="mt-4 flex items-center gap-2.5 rounded-lg border bg-card px-3 py-3 text-[13px]" data-join-joined>
              <CheckIcon className="size-4 shrink-0 text-success" aria-hidden />
              <span>This computer joined. It shows in your machines.</span>
            </div>
          ) : (
            <>
              <Input
                autoFocus
                aria-label="Pairing code"
                placeholder="XXXX-XXXX-XXXX"
                className="mt-4 font-mono tracking-wider"
                value={text}
                disabled={busy}
                spellCheck={false}
                autoCapitalize="characters"
                autoComplete="off"
                onChange={(e) => setText(e.target.value)}
              />
              {joining.error ? (
                <p role="alert" className="mt-2 text-xs text-destructive" data-join-error>
                  {joining.error}
                </p>
              ) : null}
              {busy ? (
                <p className="mt-3 flex items-center gap-2 text-xs text-muted-foreground" data-join-busy>
                  <LoaderCircleIcon className="size-3.5 animate-spin" aria-hidden /> Setting this computer up…
                </p>
              ) : null}
            </>
          )}
          <div className="mt-4 flex justify-end gap-2">
            {joined ? (
              <Button type="button" onClick={() => onOpenChange(false)}>
                Done
              </Button>
            ) : (
              <>
                <Button type="button" variant="outline" disabled={busy} onClick={() => onOpenChange(false)}>
                  Cancel
                </Button>
                <Button type="submit" disabled={busy || !ready}>
                  Join
                </Button>
              </>
            )}
          </div>
        </form>
      </DialogPopup>
    </Dialog>
  );
}
