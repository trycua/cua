// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Spaces page's list: OS (or owner) and name with who has it, status,
 * computer, size on disk ("—" when it isn't yours or isn't known), last
 * used. Name, Size and Last used sort from their headers. No row lines: a
 * fill on hover and on selection.
 *
 * With the Child Spaces experiment on, a Space's children follow it,
 * indented and collapsed under a disclosure on its row.
 */

import { Link, useNavigate } from "@tanstack/react-router";
import { ArrowDownIcon, ArrowUpIcon, ChevronRightIcon, EyeOffIcon, LoaderCircleIcon } from "lucide-react";
import { useState, type MouseEvent, type ReactNode, type RefObject } from "react";

import { useChildren } from "@/components/children/use-children";
import { OsIcon } from "@/components/os-icon";
import { StateLabel } from "@/components/state-dot";
import { DesktopTag, HomeTag, PinnedMark } from "@/components/spaces/space-tile";
import { useWindowedRows } from "@/hooks/use-windowed-rows";
import { lastUsedText, sizeCell, type SpacesSort } from "@/lib/spaces-page";
import { nestRows } from "@/lib/space-tree";
import { WINDOW_FROM } from "@/lib/windowing";
import { cn } from "@/lib/utils";

import { PeopleStack, SpaceMark } from "./avatars";
import { desktopWord, hasHideButton, hasMenu, type SpaceItem } from "./model";

export interface SpaceListProps {
  items: SpaceItem[];
  sort: SpacesSort;
  onSort: (sort: SpacesSort) => void;
  now: number;
  /** The checkbox column: the header's box, and each row's. */
  selection?: {
    header: ReactNode;
    row: (item: SpaceItem) => ReactNode;
    selected: (item: SpaceItem) => boolean;
    busy: (item: SpaceItem) => boolean;
    /** A click on a row; call `preventDefault` to keep it from opening. */
    onClick: (item: SpaceItem, e: MouseEvent) => void;
  };
  /** The row's "⋯" (on hover) and right-click menu. */
  menu?: (item: SpaceItem, children: ReactNode) => ReactNode;
  more?: (item: SpaceItem) => ReactNode;
  /** A machine's desktop row: Hide, in place of the "⋯", on hover. */
  hide?: (item: SpaceItem) => void;
  /** Every Space's id, so a parent that is only filtered out is not taken for a removed one. */
  known?: ReadonlySet<string>;
}

/** A list row's height (`h-10`), and nothing between rows. */
const ROW_PITCH = 40;

export function SpaceList({ items, sort, onSort, now, selection, menu, more, hide, known }: SpaceListProps) {
  const tree = useChildren();
  // Children show under their parent; a person folds a parent to hide what is under it.
  const [closed, setClosed] = useState<ReadonlySet<string>>(() => new Set());
  const toggle = (id: string) =>
    setClosed((o) => {
      const next = new Set(o);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  const rows = nestRows(items, tree, closed, known);
  // A long list draws only the rows on screen (`lib/windowing.ts`).
  const windowed = useWindowedRows({ enabled: rows.length >= WINDOW_FROM, rows: rows.length, perRow: 1, gap: 0, estimatePitch: () => ROW_PITCH });
  const { start: from, end: to } = windowed.window;
  const cols = cn("grid items-center gap-x-3", selection ? "grid-cols-[20px_minmax(0,1fr)_100px_140px_72px_100px_28px]" : "grid-cols-[minmax(0,1fr)_100px_140px_72px_100px_28px]");
  const head = (key: SpacesSort | null, label: string, right = false) =>
    key ? (
      <button
        type="button"
        data-sort-header={key}
        onClick={() => onSort(key)}
        className={cn("flex cursor-default items-center gap-1 text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:text-foreground", right && "justify-end", key === sort && "font-medium text-foreground")}
      >
        {label}
        {key === sort ? key === "name" ? <ArrowUpIcon className="size-3" aria-hidden /> : <ArrowDownIcon className="size-3" aria-hidden /> : null}
      </button>
    ) : (
      <span className={cn("text-xs text-muted-foreground", right && "text-right")}>{label}</span>
    );
  return (
    <div data-space-list="" role="table" aria-label="Spaces">
      <div role="row" className={cn(cols, "h-8 px-2")}>
        {selection ? <span role="columnheader">{selection.header}</span> : null}
        <span role="columnheader">{head("name", "Name")}</span>
        <span role="columnheader">{head(null, "Status")}</span>
        <span role="columnheader">{head(null, "Machine")}</span>
        <span role="columnheader">{head("size", "Size", true)}</span>
        <span role="columnheader">{head("used", "Last used")}</span>
        <span />
      </div>
      <div role="rowgroup" ref={windowed.ref as RefObject<HTMLDivElement>} style={windowed.style}>
        {rows.slice(from, to).map(({ item, depth, kids, below, orphan }) => {
          const nest = { depth, kids, below, orphan, open: !closed.has(item.space.id), toggle: () => toggle(item.space.id) };
          const row = <ListRow key={item.space.id} item={item} cols={cols} now={now} selection={selection} more={hasMenu(item) ? more?.(item) : undefined} hide={hasHideButton(item) ? () => hide?.(item) : undefined} nest={nest} />;
          return menu ? <div key={item.space.id}>{menu(item, row)}</div> : row;
        })}
      </div>
    </div>
  );
}

function ListRow({
  item,
  cols,
  now,
  selection,
  more,
  hide,
  nest,
}: {
  item: SpaceItem;
  cols: string;
  now: number;
  selection: SpaceListProps["selection"];
  more?: ReactNode;
  hide?: () => void;
  nest?: { depth: number; kids: number; below: number; orphan: boolean; open: boolean; toggle: () => void };
}) {
  const { space, state, access } = item;
  const navigate = useNavigate();
  const selected = selection?.selected(item) ?? false;
  const busy = selection?.busy(item) ?? false;
  return (
    <div
      role="row"
      data-space-row={space.id}
      data-state={state}
      data-selected={selected || undefined}
      onClick={(e) => {
        selection?.onClick(item, e);
        if (e.defaultPrevented) return;
        void navigate({ to: "/spaces/$spaceId", params: { spaceId: space.id } });
      }}
      className={cn(
        cols,
        "group/row h-10 cursor-default rounded-lg px-2 text-[13px]",
        selected ? "bg-brand/12" : "hover:bg-tile",
        (busy || state === "deleting") && "opacity-50",
      )}
    >
      {selection ? <span role="cell">{selection.row(item)}</span> : null}
      <span role="cell" className="flex min-w-0 items-center gap-2.5" style={nest?.depth ? { paddingLeft: nest.depth * 20 } : undefined}>
        {nest && nest.kids > 0 ? (
          <button
            type="button"
            aria-label={nest.open ? "Hide its child Spaces" : `Show its ${nest.below} child Space${nest.below === 1 ? "" : "s"}`}
            aria-expanded={nest.open}
            data-children-toggle={space.id}
            onClick={(e) => {
              e.stopPropagation();
              nest.toggle();
            }}
            className="-mx-1 inline-flex size-5 shrink-0 cursor-default items-center justify-center rounded-lg text-muted-foreground outline-none hover:bg-accent hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
          >
            <ChevronRightIcon className={cn("size-3.5 transition-transform", nest.open && "rotate-90")} />
          </button>
        ) : null}
        {access.kind === "mine" ? (
          <OsIcon os={space.os} className="size-3.5 shrink-0 text-muted-foreground" />
        ) : (
          <SpaceMark space={space} access={access} className="-mx-[3px] size-5 text-[10px]" />
        )}
        <Link
          to="/spaces/$spaceId"
          params={{ spaceId: space.id }}
          data-space-id={space.id}
          onClick={(e) => e.stopPropagation()}
          className="min-w-0 cursor-default truncate font-medium outline-none focus-visible:underline"
        >
          {space.name}
        </Link>
        {nest?.orphan ? (
          <span data-parent-removed="" className="shrink-0 text-[11px] text-muted-foreground">
            parent removed
          </span>
        ) : null}
        {nest && !nest.open && nest.below > 0 ? (
          <span data-children-count="" className="shrink-0 text-[11px] text-muted-foreground">
            {nest.below} child Space{nest.below === 1 ? "" : "s"}
          </span>
        ) : null}
        <PinnedMark id={space.id} />
        {item.desktop ? <DesktopTag /> : null}
        <HomeTag spaceId={item.space.id} />
        <PeopleStack access={access} />
      </span>
      <span role="cell" className="flex min-w-0 items-center">
        {busy || state === "deleting" ? (
          <span className="inline-flex items-center gap-1.5 text-xs text-muted-foreground">
            <LoaderCircleIcon className="size-3 animate-spin" aria-hidden /> Deleting
          </span>
        ) : (
          <StateLabel state={item.desktop === "off" ? "stopped" : state} space={space} word={desktopWord(item)} />
        )}
      </span>
      <span role="cell" className="truncate text-xs text-muted-foreground">
        {access.place || item.machine}
      </span>
      <span role="cell" data-size-cell="" className="text-right text-xs tabular-nums text-muted-foreground">
        {sizeCell(item.bytes, item.mine)}
      </span>
      <span role="cell" data-last-used="" className="truncate text-xs text-muted-foreground">
        {lastUsedText(space.lastUsedAt, now)}
      </span>
      <span role="cell" className="flex justify-end" onClick={(e) => e.stopPropagation()}>
        {hide ? (
          <button
            type="button"
            data-row-hide=""
            title="Hide"
            aria-label={`Hide ${space.name}`}
            onClick={hide}
            className="flex size-7 shrink-0 cursor-default items-center justify-center rounded-lg text-muted-foreground opacity-0 outline-none group-hover/row:opacity-100 hover:bg-tile hover:text-foreground focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-ring/60"
          >
            <EyeOffIcon className="size-4" aria-hidden />
          </button>
        ) : (
          more
        )}
      </span>
    </div>
  );
}
