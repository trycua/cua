// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Picking several Spaces on the Spaces page: the checkbox (on hover),
 * ⌘-click and ⇧-click (a range) anywhere on a tile or row, ⌘A for every
 * visible one, Esc to clear. Yours can be picked to delete, someone else's
 * to leave. With a pick, the toolbar row becomes the selection bar.
 */

import { CheckIcon, LoaderCircleIcon, MinusIcon, PowerIcon, XIcon } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent } from "react";

import { Button } from "@/components/ui/button";
import { canDelete } from "@/lib/spaces";
import { cn } from "@/lib/utils";

import type { SpaceItem } from "./model";

/** It can be ticked: yours to delete, or someone else's to leave; not while it goes. */
export function selectable(item: SpaceItem): boolean {
  if (item.state === "deleting" || item.state === "creating") return false;
  return item.mine ? item.access.canDelete && canDelete(item.space) : item.access.canLeave;
}

/** Why it can't be ticked (the checkbox still shows, dimmed, saying this); null when it can. */
export function unselectableReason(item: SpaceItem): string | null {
  if (selectable(item)) return null;
  if (item.state === "creating") return "It can be selected once it's created";
  if (item.state === "deleting") return "It's being deleted";
  return item.mine ? "This Space can't be deleted here" : "You can't leave this Space";
}

/** Stopped and yours to delete: what Select stopped picks. */
export const stoppedDeletable = (item: SpaceItem) => item.state === "stopped" && item.mine && selectable(item);

export interface Selection {
  ids: ReadonlySet<string>;
  has: (id: string) => boolean;
  /** The checkbox, or a ⌘/⇧ click (⇧: from the last one picked to this one). */
  toggle: (item: SpaceItem, e?: { shiftKey?: boolean }) => void;
  set: (ids: Iterable<string>) => void;
  clear: () => void;
  /** Every visible one that can be picked. */
  selectAll: () => void;
  /** Every visible stopped Space of yours. */
  selectStopped: () => void;
  /** A click on a tile or row: ⌘ or ⇧ picks it instead of opening it. */
  onClick: (item: SpaceItem, e: MouseEvent) => void;
}

/** The selection over the Spaces shown now; `paused` (a dialog open) leaves ⌘A and Esc alone. */
export function useSelection(shown: readonly SpaceItem[], paused: boolean): Selection {
  const [ids, setIds] = useState<ReadonlySet<string>>(new Set());
  const anchor = useRef<string | null>(null);
  // Gone from view (filtered away, deleted): no longer picked.
  useEffect(() => {
    const visible = new Set(shown.map((i) => i.space.id));
    setIds((cur) => ([...cur].every((id) => visible.has(id)) ? cur : new Set([...cur].filter((id) => visible.has(id)))));
  }, [shown]);
  const toggle = useCallback(
    (item: SpaceItem, e?: { shiftKey?: boolean }) => {
      const id = item.space.id;
      setIds((cur) => {
        const next = new Set(cur);
        const order = shown.map((i) => i.space.id);
        const from = anchor.current ? order.indexOf(anchor.current) : -1;
        if (e?.shiftKey && from >= 0) {
          const to = order.indexOf(id);
          const [a, b] = from < to ? [from, to] : [to, from];
          for (const i of shown.slice(a, b + 1)) if (selectable(i)) next.add(i.space.id);
        } else if (next.has(id)) next.delete(id);
        else if (selectable(item)) next.add(id);
        return next;
      });
      anchor.current = id;
    },
    [shown],
  );
  const selectAll = useCallback(() => setIds(new Set(shown.filter(selectable).map((i) => i.space.id))), [shown]);
  const selectStopped = useCallback(() => setIds(new Set(shown.filter(stoppedDeletable).map((i) => i.space.id))), [shown]);
  const clear = useCallback(() => setIds(new Set()), []);
  useEffect(() => {
    if (paused) return;
    const onKey = (e: KeyboardEvent) => {
      const typing = (e.target as HTMLElement | null)?.closest?.("input, textarea, select, [contenteditable=true], [role=menu], [role=dialog]");
      if (typing) return;
      if (e.key === "Escape" && ids.size) {
        e.preventDefault();
        clear();
      } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "a") {
        e.preventDefault();
        selectAll();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [paused, ids.size, clear, selectAll]);
  return useMemo(
    () => ({
      ids,
      has: (id: string) => ids.has(id),
      toggle,
      set: (next: Iterable<string>) => setIds(new Set(next)),
      clear,
      selectAll,
      selectStopped,
      onClick: (item: SpaceItem, e: MouseEvent) => {
        if (!(e.metaKey || e.ctrlKey || e.shiftKey)) return;
        e.preventDefault();
        e.stopPropagation();
        toggle(item, e);
      },
    }),
    [ids, toggle, clear, selectAll, selectStopped],
  );
}

/** A tile's or row's checkbox. Over a picture it sits on a dark scrim; in the list it is plain. */
export function SelectBox({ item, selection, onPicture = false, className }: { item: SpaceItem; selection: Selection; onPicture?: boolean; className?: string }) {
  const on = selection.has(item.space.id);
  const can = selectable(item);
  const why = can || on ? null : unselectableReason(item);
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={on}
      aria-label={`Select ${item.space.name}`}
      aria-disabled={!can || undefined}
      aria-description={why ?? undefined}
      title={why ?? undefined}
      data-select={item.space.id}
      // Over a tile's picture: cut out of a live tile's native video while it shows (`lib/video-slots.ts`).
      data-video-hole={onPicture ? "" : undefined}
      onClick={(e) => {
        e.preventDefault();
        e.stopPropagation();
        if (can || on) selection.toggle(item, e);
      }}
      className={cn(
        "flex size-4 cursor-default items-center justify-center rounded-[4px] outline-none transition-opacity focus-visible:ring-2 focus-visible:ring-ring/60",
        on ? "bg-brand text-white" : onPicture ? "bg-black/35 text-transparent shadow-[inset_0_0_0_1.5px_rgba(255,255,255,0.9)] backdrop-blur-sm" : "text-transparent shadow-[inset_0_0_0_1.5px_var(--color-input)]",
        !can && !on && "opacity-40",
        className,
      )}
    >
      <CheckIcon className="size-3" strokeWidth={3} />
    </button>
  );
}

/** The list header's box: all, some (a dash) or none of the visible ones. */
export function SelectAllBox({ items, selection }: { items: readonly SpaceItem[]; selection: Selection }) {
  const can = items.filter(selectable);
  const all = can.length > 0 && can.every((i) => selection.has(i.space.id));
  const some = !all && items.some((i) => selection.has(i.space.id));
  return (
    <button
      type="button"
      role="checkbox"
      aria-checked={all ? true : some ? "mixed" : false}
      aria-label="Select all"
      data-select-all=""
      disabled={!can.length}
      onClick={() => (all || some ? selection.clear() : selection.selectAll())}
      className={cn(
        "flex size-4 cursor-default items-center justify-center rounded-[4px] outline-none focus-visible:ring-2 focus-visible:ring-ring/60 disabled:opacity-40",
        all || some ? "bg-brand text-white" : "shadow-[inset_0_0_0_1.5px_var(--color-input)]",
      )}
    >
      {all ? <CheckIcon className="size-3" strokeWidth={3} /> : some ? <MinusIcon className="size-3" strokeWidth={3} /> : null}
    </button>
  );
}

export interface SelectionBarProps {
  picked: readonly SpaceItem[];
  /** While a bulk delete or leave runs: the core's "Deleting 1 of 3…". */
  progress: string | null;
  onClear: () => void;
  onPower: (on: boolean) => void;
  onRemove: () => void;
}

/**
 * In place of the toolbar while something is picked: the count, Turn on or
 * Turn off (by what is picked; only when every one may be), Delete (Leave
 * when only others' are picked), and × to clear. While it runs, the count
 * through.
 */
export function SelectionBar({ picked, progress, onClear, onPower, onRemove }: SelectionBarProps) {
  const powerable = picked.length > 0 && picked.every((i) => i.access.canPower && Boolean(i.space.power) && (i.state === "running" || i.state === "stopped"));
  const anyRunning = picked.some((i) => i.state === "running");
  const onlyOthers = picked.length > 0 && picked.every((i) => !i.mine);
  return (
    <div data-selection-bar="" className="mb-5 flex h-9 items-center gap-1 rounded-lg bg-brand/12 pr-1.5 pl-1">
      {progress ? (
        <p data-bulk-progress="" className="flex items-center gap-2 px-2 text-[13px] font-medium">
          <LoaderCircleIcon className="size-3.5 animate-spin text-muted-foreground" aria-hidden />
          {progress}
        </p>
      ) : (
        <>
          <Button size="icon-sm" variant="ghost" aria-label="Clear selection" data-clear-selection="" className="hover:bg-foreground/8" onClick={onClear}>
            <XIcon className="size-3.5" />
          </Button>
          <p data-selected-count="" className="text-[13px] font-medium">
            {picked.length} selected
          </p>
          <div className="ml-auto flex items-center gap-1">
            {powerable ? (
              <Button size="sm" variant="ghost" data-bulk-power={anyRunning ? "off" : "on"} className="hover:bg-foreground/8" onClick={() => onPower(!anyRunning)}>
                <PowerIcon className="size-3.5" /> {anyRunning ? "Turn off" : "Turn on"}
              </Button>
            ) : null}
            <Button size="sm" variant="ghost" data-bulk-remove={onlyOthers ? "leave" : "delete"} className={cn("hover:bg-foreground/8", !onlyOthers && "text-destructive")} onClick={onRemove}>
              {onlyOthers ? "Leave" : "Delete"}
            </Button>
          </div>
        </>
      )}
    </div>
  );
}
