// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useShareSheet } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { cn } from "@/lib/utils";

/**
 * "Share": one line per person with a role menu, an email field and Done,
 * as the SwiftUI app's `ShareSheetView`. Every word and step is the app
 * core's (`share.view`); sharing asks for presence in the daemon before it
 * reaches the relay.
 */
export default function ShareSheet() {
  const { session, sharing } = useShareSheet();
  if (!session || !sharing) return null;
  const v = session.view;
  const locked = v.busy || v.disabledReason !== null;
  const roles = v.roles.map((r) => ({ value: r.id, label: r.label }));
  const send = sharing.send.bind(sharing);
  return (
    <Dialog open onOpenChange={(open) => !open && sharing.close()}>
      <DialogPopup data-share-sheet className="w-[min(520px,calc(100vw-2rem))]">
        <div className="px-5 pt-4 pb-4">
          <DialogTitle className="text-[15px] font-semibold" data-share-title>
            {v.title}
          </DialogTitle>
          {v.disabledReason ? (
            <p className="mt-1.5 text-[13px] text-muted-foreground" data-share-disabled>
              {v.disabledReason}
            </p>
          ) : null}

          <form
            className="mt-4 flex items-center gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              void send({ type: "submit" });
            }}
          >
            <Input
              aria-label={v.whoPlaceholder}
              placeholder={v.whoPlaceholder}
              value={v.who}
              disabled={v.disabledReason !== null}
              onChange={(e) => void send({ type: "set-who", who: e.target.value })}
              data-share-who
            />
            <span data-share-role={v.role}>
              <Select aria-label="Role" className="h-8 min-w-28" value={v.role} options={roles} onValueChange={(role) => void send({ type: "set-role", role })} />
            </span>
            <Button type="submit" disabled={!v.canShare} data-share-submit>
              {v.shareLabel}
            </Button>
          </form>
          {v.hint ? (
            <p className="mt-1.5 text-xs text-muted-foreground" data-share-hint>
              {v.hint}
            </p>
          ) : null}

          <ul className="mt-4 divide-y overflow-hidden rounded-lg border bg-card">
            {v.rows.length === 0 ? (
              <li className="px-4 py-3 text-[13px] text-muted-foreground" data-share-empty>
                {v.emptyText}
              </li>
            ) : (
              v.rows.map((row) => (
                <li key={row.who} className="flex items-center gap-2.5 px-4 py-2" data-share-row={row.who} data-role={row.role} data-connected={row.connected || undefined}>
                  <span
                    className={cn("size-1.5 shrink-0 rounded-full", row.connected ? "bg-emerald-500" : "bg-muted-foreground/40")}
                    title={row.connected ? "Connected now" : undefined}
                  />
                  <span className="min-w-0 flex-1 truncate text-[13px]">{row.who}</span>
                  {locked ? (
                    <span className="text-xs text-muted-foreground">{roles.find((r) => r.value === row.role)?.label ?? row.role}</span>
                  ) : (
                    <Select
                      aria-label={`Role for ${row.who}`}
                      className="min-w-28"
                      value={row.role}
                      options={roles}
                      onValueChange={(role) => void send({ type: "change-role", who: row.who, role })}
                    />
                  )}
                  <Button size="sm" variant="ghost" disabled={v.busy} onClick={() => void send({ type: "remove", who: row.who })} data-share-remove>
                    {v.removeLabel}
                  </Button>
                </li>
              ))
            )}
          </ul>
          {v.error ? (
            <p className="mt-2 text-[13px] text-destructive" data-share-error>
              {v.error}
            </p>
          ) : null}
        </div>
        <div className="flex items-center justify-end gap-2 border-t bg-muted/60 px-5 py-3">
          <Button onClick={() => sharing.close()}>{v.doneLabel}</Button>
        </div>
      </DialogPopup>
    </Dialog>
  );
}
