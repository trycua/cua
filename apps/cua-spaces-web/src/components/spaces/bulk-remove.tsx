// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Deleting and leaving Spaces together: one confirm in the core's words
 * (`spaces.bulkConfirm`: each Space with its size, grouped by computer or
 * by Delete and Leave, what it frees, which running ones are turned off
 * first, who loses access), then the store's `deleteSpaces` / `leaveSpaces`
 * with the core's count through ("Deleting 1 of 3…"), then its toast
 * ("Deleted 3 Spaces. 29 GB freed.").
 */

import { AlertDialog as AlertDialogPrimitive } from "@base-ui/react/alert-dialog";
import { useRef, useState } from "react";

import { bulkConfirm, bulkDone, bulkProgress, useBridge, useSpaces, type BulkConfirmView, type BulkItem, type BulkResult } from "@/bridge";
import { OsIcon } from "@/components/os-icon";
import { AlertDialog, AlertDialogPopup } from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import { avatarColor, initial } from "@/lib/spaces-page";
import { cn } from "@/lib/utils";

import { SpaceMark } from "./avatars";
import type { SpaceItem } from "./model";

/** What the core's confirm reads for one picked Space. */
export function bulkItem(item: SpaceItem): BulkItem {
  return {
    id: item.space.id,
    name: item.space.name,
    os: item.space.os,
    running: item.state === "running",
    bytes: item.mine ? item.bytes : null,
    mine: item.mine,
    place: item.access.place || item.machine,
    team: item.access.kind === "team",
    people: item.mine ? item.access.people.map((p) => ({ who: p.who, name: p.name ?? null })) : [],
  };
}

export interface BulkRemove {
  /** The confirm on screen, and the Spaces it asks about. */
  view: BulkConfirmView | null;
  asked: readonly SpaceItem[];
  /** Asks about these Spaces (nothing when none may go). */
  ask: (items: readonly SpaceItem[]) => void;
  cancel: () => void;
  /** Yes: delete and leave them. */
  confirm: () => Promise<void>;
  /** While it runs: the bar's "Deleting 1 of 3…". */
  progress: string | null;
  /** The ids it is working on now. */
  running: ReadonlySet<string>;
}

/**
 * The bulk confirm and its run. `present` is the ids still listed: one that
 * left the list while the run goes counts as finished. `onDone` runs once
 * every result is in (the selection clears).
 */
export function useBulkRemove(present: ReadonlySet<string>, onDone: (results: BulkResult[]) => void): BulkRemove {
  const { core } = useBridge();
  const { deleteSpaces, leaveSpaces } = useSpaces();
  const [asked, setAsked] = useState<{ items: BulkItem[]; picked: readonly SpaceItem[]; view: BulkConfirmView } | null>(null);
  const [run, setRun] = useState<{ items: BulkItem[]; finished: string[] } | null>(null);
  const busy = useRef(false);
  const finished = run ? [...new Set([...run.finished, ...run.items.filter((i) => !present.has(i.id)).map((i) => i.id)])] : [];
  const progress = run && core.status === "ready" ? (bulkProgress(core, run.items, finished) ?? null) : null;
  return {
    view: asked?.view ?? null,
    asked: asked?.picked ?? [],
    progress,
    running: new Set(run?.items.filter((i) => !finished.includes(i.id)).map((i) => i.id) ?? []),
    ask: (picked) => {
      if (core.status !== "ready" || busy.current) return;
      const items = picked.map(bulkItem);
      const view = bulkConfirm(core, items);
      if (view) setAsked({ items, picked, view });
    },
    cancel: () => setAsked(null),
    confirm: async () => {
      if (!asked || busy.current) return;
      busy.current = true;
      const { items, view } = asked;
      setAsked(null);
      setRun({ items, finished: [] });
      const done = (r: BulkResult[]) => setRun((cur) => (cur ? { ...cur, finished: [...cur.finished, ...r.map((x) => x.id)] } : cur));
      // Yours are deleted, others' left; both at once, each answering per Space.
      const [deleted, left] = await Promise.all([
        view.deleteIds.length ? deleteSpaces(view.deleteIds).then((r) => (done(r), r)) : Promise.resolve([] as BulkResult[]),
        view.leaveIds.length ? leaveSpaces(view.leaveIds).then((r) => (done(r), r)) : Promise.resolve([] as BulkResult[]),
      ]);
      const results = [...deleted, ...left];
      setRun(null);
      busy.current = false;
      const said = bulkDone(core, items, results);
      const failed = results.filter((r) => !r.ok);
      toast(said.text, {
        type: said.tone === "ok" ? "success" : "error",
        description: failed.length ? failed.map((r) => `${items.find((i) => i.id === r.id)?.name ?? r.id}: ${r.error ?? "failed"}`).join("\n") : undefined,
        group: "spaces-bulk",
      });
      onDone(results);
    },
  };
}

/** The confirm, as the core lays it out. */
export function BulkConfirmDialog({ view, items = [], onCancel, onConfirm }: { view: BulkConfirmView; items?: readonly SpaceItem[]; onCancel: () => void; onConfirm: () => void }) {
  return (
    <AlertDialog open onOpenChange={(o) => !o && onCancel()}>
      <AlertDialogPopup data-bulk-confirm="" className="w-[min(460px,calc(100vw-2rem))] rounded-lg border-0">
        <div className="px-5 pt-5">
          <AlertDialogPrimitive.Title className="text-[15px] font-semibold">{view.title}</AlertDialogPrimitive.Title>
          <AlertDialogPrimitive.Description data-bulk-body="" className="mt-1.5 text-[13px] text-muted-foreground">
            {view.body}
          </AlertDialogPrimitive.Description>
        </div>
        <div className="mx-5 mt-4 max-h-64 overflow-y-auto rounded-lg bg-tile py-1">
          {view.groups.map((g, gi) => (
            <div key={g.title ?? gi} data-bulk-group={g.title ?? ""}>
              {g.title ? <p className="px-3 pt-2 pb-1 text-2xs font-medium text-muted-foreground">{g.title}</p> : null}
              {g.rows.map((r) => (
                <div key={r.id} data-bulk-row={r.id} className="flex h-8 items-center gap-2.5 px-3 text-[13px]">
                  {(() => {
                    // Someone else's: their avatar, as on the page; yours: the OS.
                    const it = items.find((i) => i.space.id === r.id);
                    return it && !it.mine ? (
                      <SpaceMark space={it.space} access={it.access} className="size-4 text-[9px]" />
                    ) : (
                      <OsIcon os={(["macos", "linux", "windows"].includes(r.os) ? r.os : "unknown") as "macos"} className="size-3.5 shrink-0 text-muted-foreground" />
                    );
                  })()}
                  <span className="min-w-0 truncate font-medium">{r.name}</span>
                  {r.status ? (
                    <span className="flex shrink-0 items-center gap-1.5 text-xs text-muted-foreground">
                      <span aria-hidden className="size-1.5 rounded-full bg-success" /> {r.status}
                    </span>
                  ) : null}
                  {r.people.length ? (
                    <span className="inline-flex shrink-0 items-center">
                      {r.people.slice(0, 3).map((p, i) => (
                        <span key={`${p}:${i}`} aria-hidden className={cn("flex size-4 items-center justify-center rounded-full text-[9px] font-semibold text-white ring-2 ring-popover", avatarColor(p), i > 0 && "-ml-1")}>
                          {initial(p)}
                        </span>
                      ))}
                      {r.people.length > 3 ? <span className="ml-1 text-[11px] text-muted-foreground tabular-nums">+{r.people.length - 3}</span> : null}
                    </span>
                  ) : null}
                  <span className="ml-auto shrink-0 text-xs text-muted-foreground tabular-nums">{r.detail ?? "—"}</span>
                </div>
              ))}
            </div>
          ))}
        </div>
        <div className="mt-4 flex items-center gap-2 bg-muted/60 px-5 py-3">
          {view.frees ? (
            <p data-frees="" className="text-[13px]">
              {view.freesLabel} <span className="font-semibold tabular-nums">{view.frees}</span>
            </p>
          ) : null}
          <div className="ml-auto flex gap-2">
            <AlertDialogPrimitive.Close render={<Button variant="outline" />}>{view.cancel}</AlertDialogPrimitive.Close>
            <Button variant={view.destructive ? "destructive" : "default"} data-confirm-bulk="" onClick={onConfirm}>
              {view.button}
            </Button>
          </div>
        </div>
      </AlertDialogPopup>
    </AlertDialog>
  );
}
