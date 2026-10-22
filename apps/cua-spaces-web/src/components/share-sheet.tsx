// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect } from "react";

import {
  teamName,
  useFeatureRecorded,
  useSession,
  useShareSheet,
  useTeamChoices,
} from "@/bridge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Select } from "@/components/ui/select";
import { UpsellPrompt } from "@/components/upsell-prompt";
import { cn } from "@/lib/utils";

/**
 * "Share": one line per person with a role menu, an email field and Done,
 * as the SwiftUI app's `ShareSheetView`. Every word and step is the app
 * core's (`share.view`); sharing asks for presence in the daemon before it
 * reaches the relay. "Share with team" offers the account's teams that can
 * take machines (owner or admin, Teams plan), the current team first: it
 * shares with `team:<id>`, as typing it would, as "Can view" or "Can edit".
 * Sharing is for teams: without one the sheet says so and links to Teams.
 */
export default function ShareSheet() {
  const { session, sharing } = useShareSheet();
  useFeatureRecorded("share_open", Boolean(session));
  const choices = useTeamChoices(Boolean(session?.input.signedIn && session.input.shareable));
  const { openExternal } = useSession();
  const { hasTeam } = choices;
  useEffect(() => {
    sharing?.setHasTeam(hasTeam);
  }, [sharing, hasTeam, session?.input.spaceId]);
  if (!session || !sharing) return null;
  const v = session.view;
  const gate = session.gate;
  const closed = v.disabledReason !== null || gate !== null;
  const locked = v.busy || closed;
  const roles = v.roles.map((r) => ({ value: r.id, label: r.label }));
  const send = sharing.send.bind(sharing);
  const shared = new Set(v.rows.map((r) => r.who));
  const offered = choices.teams.filter((t) => !shared.has(`team:${t.id}`));
  const shareWithTeam = async (id: string) => {
    await send({ type: "set-who", who: `team:${id}` });
    await send({ type: "submit" });
  };
  // `team:<id>` rows read as the team's name.
  const label = (who: string) => {
    const t = who.startsWith("team:") ? choices.all.find((x) => x.id === who.slice(5)) : undefined;
    return t ? `${teamName(t)} (team)` : who;
  };
  return (
    <Dialog open onOpenChange={(open) => !open && sharing.close()}>
      <DialogPopup data-share-sheet className="w-[min(520px,calc(100vw-2rem))]">
        <div className="px-5 pt-4 pb-4">
          <DialogTitle className="text-[15px] font-semibold" data-share-title>
            {v.title}
          </DialogTitle>
          {v.disabledReason || gate ? (
            <p className="mt-1.5 text-[13px] text-muted-foreground" data-share-disabled>
              {v.disabledReason ?? gate?.reason}
            </p>
          ) : null}
          {!v.disabledReason && gate ? (
            <Button size="sm" variant="outline" className="mt-2" onClick={() => void openExternal(gate.action.url).catch(() => {})} data-share-action>
              {gate.action.label}
            </Button>
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
              disabled={closed}
              onChange={(e) => void send({ type: "set-who", who: e.target.value })}
              data-share-who
            />
            <span data-share-role={v.role}>
              <Select aria-label="Role" className="h-8 min-w-28" value={v.role} options={roles} onValueChange={(role) => void send({ type: "set-role", role })} />
            </span>
            <Button type="submit" disabled={!v.canShare || closed} data-share-submit>
              {v.shareLabel}
            </Button>
          </form>
          {offered.length > 0 && !closed ? (
            <div className="mt-3 flex flex-wrap items-center gap-2" data-share-teams>
              {offered.map((t) => (
                <Button key={t.id} size="sm" variant="outline" disabled={v.busy} onClick={() => void shareWithTeam(t.id)} data-share-team={t.id}>
                  Share with {teamName(t)}
                </Button>
              ))}
            </div>
          ) : null}
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
                  <span className="min-w-0 flex-1 truncate text-[13px]">{label(row.who)}</span>
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
          <UpsellPrompt surface="share" active={v.rows.some((r) => !r.who.startsWith("team:"))} className="mt-3" />
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
