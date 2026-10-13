// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { ChevronDownIcon, ChevronRightIcon, CookieIcon, DatabaseIcon, EyeOffIcon, FileIcon, FolderIcon, GlobeIcon, KeyRoundIcon, SearchIcon } from "lucide-react";
import type { ReactNode } from "react";

import type { TeleportStore, Tri, VaultAction, VaultRow, VaultView } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { vaultLines, type VaultLine } from "@/lib/keyvault";

export const KIND_ICON = { cookie: CookieIcon, local_storage: DatabaseIcon, password: KeyRoundIcon, file: FileIcon } as const;

/**
 * The app's saved Keyvault items, to choose from as the review's source
 * (the SwiftUI review's `SavedItemsChoice`): grouped by site with
 * checkboxes and search, the names behind Touch ID. The core's vault list
 * keeps the selection; the review sends exactly it.
 */
export function SavedItems({ view, query, teleport }: { view: VaultView; query: string; teleport: TeleportStore }) {
  const send = (action: VaultAction) => teleport.sendVault(action);
  if (view.namesHidden) {
    return (
      <div className="flex items-center gap-3 px-4 py-3" data-saved-hidden>
        <EyeOffIcon className="size-4 shrink-0 text-muted-foreground" />
        <p className="min-w-0 flex-1 text-[13px] text-muted-foreground">{view.hiddenNote}</p>
        <Button size="sm" variant="outline" onClick={() => void teleport.showNames()}>
          {view.showNamesLabel}
        </Button>
      </div>
    );
  }
  // The apps' own lines (one app here) are left out: the review is about its items.
  const lines = vaultLines(view).filter((l) => l.type !== "app");
  return (
    <>
      <div className="flex items-center gap-2 border-b px-4 py-1.5">
        <SearchIcon className="size-3.5 shrink-0 text-muted-foreground" />
        <input
          aria-label="Search saved items"
          placeholder="Search saved items"
          value={query}
          onChange={(e) => send({ type: "query", text: e.target.value })}
          className="h-7 min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-muted-foreground"
        />
        <span className="text-xs text-muted-foreground tabular-nums" data-saved-summary>
          {view.selection.count} of {view.shown} selected
        </span>
        <button type="button" className="text-xs text-brand hover:underline" onClick={() => send({ type: "select-all" })}>
          All
        </button>
        <button type="button" className="text-xs text-brand hover:underline" onClick={() => send({ type: "clear" })}>
          None
        </button>
      </div>
      {view.emptyText ? <p className="px-4 py-3 text-[13px] text-muted-foreground">{view.emptyText}</p> : null}
      <ul className="max-h-52 divide-y overflow-y-auto" data-saved-list>
        {lines.map((line) => (
          <li key={line.key}>
            <SavedLine line={line} send={send} />
          </li>
        ))}
      </ul>
    </>
  );
}

const triChecked = (t: Tri) => t === "on";

function SavedLine({ line, send }: { line: VaultLine; send: (a: VaultAction) => void }) {
  switch (line.type) {
    case "site":
      return (
        <Group
          selected={line.site.selected}
          open={line.site.open}
          label={`Send ${line.site.site}`}
          icon={<GlobeIcon className="size-3.5 text-muted-foreground" />}
          title={line.site.site}
          detail={line.site.counts}
          onSelect={() => send({ type: "toggle-group", key: line.site.key })}
          onOpen={() => send({ type: "toggle-open", key: line.site.key })}
        />
      );
    case "files":
      return (
        <Group
          selected={line.files.selected}
          open={line.files.open}
          label="Send Files"
          icon={<FolderIcon className="size-3.5 text-muted-foreground" />}
          title="Files"
          detail={`${line.files.count} file${line.files.count === 1 ? "" : "s"}`}
          onSelect={() => send({ type: "toggle-group", key: line.files.key })}
          onOpen={() => send({ type: "toggle-open", key: line.files.key })}
        />
      );
    case "item":
      return <Item row={line.row} onToggle={() => send({ type: "toggle", id: line.row.id })} />;
    case "app":
      return null;
  }
}

function Group({
  selected,
  open,
  label,
  icon,
  title,
  detail,
  onSelect,
  onOpen,
}: {
  selected: Tri;
  open: boolean;
  label: string;
  icon: ReactNode;
  title: string;
  detail: string;
  onSelect: () => void;
  onOpen: () => void;
}) {
  const Chevron = open ? ChevronDownIcon : ChevronRightIcon;
  return (
    <div className="flex items-center gap-2 px-4 py-2" data-saved-group={title}>
      <Checkbox aria-label={label} checked={triChecked(selected)} indeterminate={selected === "mixed"} onCheckedChange={onSelect} />
      <button type="button" aria-label={open ? "Collapse" : "Expand"} aria-expanded={open} onClick={onOpen} className="text-muted-foreground">
        <Chevron className="size-3.5" />
      </button>
      {icon}
      <span className="truncate text-[13px]">{title}</span>
      <span className="truncate text-xs text-muted-foreground">{detail}</span>
    </div>
  );
}

function Item({ row, onToggle }: { row: VaultRow; onToggle: () => void }) {
  const Icon = KIND_ICON[row.kind] ?? FileIcon;
  return (
    <label className="flex cursor-default items-center gap-2 py-2 pr-4 pl-10" data-saved-item={row.id}>
      <Checkbox aria-label={`Send ${row.title}`} checked={row.selected} onCheckedChange={onToggle} />
      <Icon className="size-3.5 shrink-0 text-muted-foreground" aria-label={row.kindLabel} />
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[13px]">{row.title}</span>
        {row.subtitle ? <span className="block truncate text-xs text-muted-foreground">{row.subtitle}</span> : null}
      </span>
      <span className="shrink-0 text-xs text-muted-foreground">{row.updated}</span>
    </label>
  );
}
