// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { LegendList } from "@legendapp/list/react";
import { ChevronDownIcon, ChevronRightIcon, EyeOffIcon, FileIcon, FolderIcon, GlobeIcon, LockIcon, LockOpenIcon, SearchIcon, Trash2Icon } from "lucide-react";

import type { KvLock, Tri, VaultAction, VaultApp, VaultRow, VaultSelection, VaultView } from "@/bridge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { KIND_ICON } from "@/components/teleport/saved-items";
import { vaultLines, type VaultLine } from "@/lib/keyvault";
import { cn } from "@/lib/utils";

interface LockActions {
  /** Locks items (narrowing: no prompt, no Touch ID). */
  lock: (ids: string[]) => void;
  /** Asks, then Touch ID once, for these items (`name`: the one item's name). */
  unlock: (ids: string[], name?: string) => void;
}

/**
 * The vault list, as the SwiftUI app draws it: apps, their sites and items
 * with a lock each; checkboxes select an item, a site or a whole app for
 * the batch bar (lock, unlock, delete). Everything shown, the grouping, the
 * selection and the search are the core's `VaultView`; a lock only asks the
 * host. Hidden names (until Touch ID) say so and offer to show them.
 */
export function VaultList({
  view,
  query,
  blocked,
  busy,
  send,
  actions,
  onShowItems,
  onDelete,
}: {
  view: VaultView;
  query: string;
  /** The Keyvault is off or locked: unlocking items waits. */
  blocked: boolean;
  busy: boolean;
  send: (action: VaultAction) => void;
  actions: LockActions;
  onShowItems: () => void;
  onDelete: (ids: string[]) => void;
}) {
  const lines = vaultLines(view);
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="mx-auto w-full max-w-3xl px-8">
        <div className="relative mb-3">
          <SearchIcon className="pointer-events-none absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input value={query} onChange={(e) => send({ type: "query", text: e.target.value })} placeholder={view.searchPrompt} aria-label="Search Keyvault" className="pl-8" />
        </div>
        {view.namesHidden ? (
          <div className="mb-3 flex items-center gap-3 rounded-xl border bg-brand-surface/60 px-4 py-2.5" data-vault-hidden>
            <EyeOffIcon className="size-4 shrink-0 text-muted-foreground" />
            <p className="min-w-0 flex-1 text-[13px] text-muted-foreground">{view.hiddenNote}</p>
            <Button size="sm" variant="outline" disabled={busy} onClick={onShowItems} data-vault-show-items>
              {view.showNamesLabel}
            </Button>
          </div>
        ) : null}
        <div className="flex h-8 items-center justify-between px-1 text-xs text-muted-foreground">
          <span data-vault-summary>{view.selection.count > 0 ? view.selection.title : `${view.total} items in ${view.apps.length} apps`}</span>
          {view.canSelectAll ? (
            <button type="button" className="text-brand hover:underline" onClick={() => send({ type: view.selection.count === view.shown ? "clear" : "select-all" })}>
              {view.selection.count === view.shown ? "Select None" : "Select All"}
            </button>
          ) : null}
        </div>
      </div>
      <div className="min-h-0 flex-1">
        {view.emptyText ? (
          <p className="mx-auto max-w-3xl px-9 py-6 text-[13px] text-muted-foreground" data-vault-empty>
            {view.emptyText}
          </p>
        ) : (
          <LegendList<VaultLine>
            data={lines}
            keyExtractor={(line) => line.key}
            getItemType={(line) => line.type}
            estimatedItemSize={40}
            recycleItems={false}
            className="h-full"
            contentContainerStyle={{ maxWidth: "48rem", marginInline: "auto", paddingInline: "2rem", paddingBottom: "5rem" }}
            renderItem={({ item: line }) => <Line line={line} send={send} actions={actions} blocked={blocked || busy} />}
          />
        )}
      </div>
      {view.selection.count > 0 ? <BatchBar selection={view.selection} blocked={blocked} busy={busy} send={send} actions={actions} onDelete={onDelete} /> : null}
    </div>
  );
}

function Line({ line, send, actions, blocked }: { line: VaultLine; send: (a: VaultAction) => void; actions: LockActions; blocked: boolean }) {
  switch (line.type) {
    case "app":
      return (
        <GroupRow
          indent={0}
          selected={line.app.selected}
          open={line.app.open}
          label={line.app.name}
          icon={<AppMonogram app={line.app} />}
          title={<span className="text-[13px] font-semibold">{line.app.name}</span>}
          detail={line.app.summary}
          updated={line.app.updated}
          lock={{ state: line.app.lock, name: line.app.name, unlockIds: line.app.unlockIds, lockIds: line.app.lockIds }}
          onSelect={() => send({ type: "toggle-group", key: line.app.key })}
          onOpen={() => send({ type: "toggle-open", key: line.app.key })}
          actions={actions}
          blocked={blocked}
        />
      );
    case "site":
      return (
        <GroupRow
          indent={1}
          selected={line.site.selected}
          open={line.site.open}
          label={line.site.site}
          icon={<GlobeIcon className="size-4 text-muted-foreground" />}
          title={<span className="text-[13px]">{line.site.site}</span>}
          detail={line.site.counts}
          updated={line.site.updated}
          lock={{ state: line.site.lock, name: line.site.site, unlockIds: line.site.unlockIds, lockIds: line.site.lockIds }}
          onSelect={() => send({ type: "toggle-group", key: line.site.key })}
          onOpen={() => send({ type: "toggle-open", key: line.site.key })}
          actions={actions}
          blocked={blocked}
        />
      );
    case "files":
      return (
        <GroupRow
          indent={1}
          selected={line.files.selected}
          open={line.files.open}
          label="files"
          icon={<FolderIcon className="size-4 text-muted-foreground" />}
          title={<span className="text-[13px]">Files</span>}
          detail={`${line.files.count} file${line.files.count === 1 ? "" : "s"}`}
          updated=""
          lock={{ state: line.files.lock, name: "files", unlockIds: line.files.unlockIds, lockIds: line.files.lockIds }}
          onSelect={() => send({ type: "toggle-group", key: line.files.key })}
          onOpen={() => send({ type: "toggle-open", key: line.files.key })}
          actions={actions}
          blocked={blocked}
        />
      );
    case "item":
      return <ItemRow row={line.row} indent={line.indent} send={send} actions={actions} blocked={blocked} />;
  }
}

const triChecked = (t: Tri) => t === "on";

function AppMonogram({ app }: { app: VaultApp }) {
  return (
    <span aria-hidden className="flex size-5 items-center justify-center rounded-[5px] bg-foreground/10 text-[11px] font-semibold">
      {app.name.slice(0, 1).toUpperCase()}
    </span>
  );
}

interface LockSpec {
  state: KvLock;
  name: string;
  unlockIds: string[];
  lockIds: string[];
}

function GroupRow({
  indent,
  selected,
  open,
  label,
  icon,
  title,
  detail,
  updated,
  lock,
  onSelect,
  onOpen,
  actions,
  blocked,
}: {
  indent: 0 | 1;
  selected: Tri;
  open: boolean;
  label: string;
  icon: React.ReactNode;
  title: React.ReactNode;
  detail: string;
  updated: string;
  lock: LockSpec;
  onSelect: () => void;
  onOpen: () => void;
  actions: LockActions;
  blocked: boolean;
}) {
  const Chevron = open ? ChevronDownIcon : ChevronRightIcon;
  return (
    <div
      data-vault-group={label}
      // The parity spec reads each app group by its name.
      data-vault-app={indent === 0 ? label : undefined}
      onClick={onOpen}
      className={cn("flex h-10 items-center gap-2.5 rounded-lg px-3 hover:bg-foreground/[0.04]", indent === 1 && "ml-6")}
    >
      <span onClick={(e) => e.stopPropagation()}>
        <Checkbox aria-label={`Select ${label}`} checked={triChecked(selected)} indeterminate={selected === "mixed"} onCheckedChange={onSelect} />
      </span>
      <Chevron className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
      {icon}
      <span className="min-w-0 truncate">{title}</span>
      <span className="min-w-0 truncate text-xs text-muted-foreground">{detail}</span>
      <span className="flex-1" />
      <span className="shrink-0 text-xs text-muted-foreground">{updated}</span>
      <LockButton {...lock} actions={actions} blocked={blocked} />
    </div>
  );
}

function ItemRow({ row, indent, send, actions, blocked }: { row: VaultRow; indent: number; send: (a: VaultAction) => void; actions: LockActions; blocked: boolean }) {
  const Icon = KIND_ICON[row.kind] ?? FileIcon;
  const toggle = () => send({ type: "toggle", id: row.id });
  return (
    <div
      role="row"
      aria-selected={row.selected}
      data-vault-item={row.id}
      onClick={toggle}
      className={cn("mb-px flex h-11 items-center gap-2.5 rounded-lg px-3 transition-colors hover:bg-foreground/[0.04]", row.selected && "bg-brand-surface hover:bg-brand-surface")}
      style={{ marginLeft: `${(indent - 1) * 1.5}rem` }}
    >
      <span onClick={(e) => e.stopPropagation()}>
        <Checkbox aria-label={`Select ${row.title}`} checked={row.selected} onCheckedChange={toggle} />
      </span>
      <Icon className="size-4 shrink-0 text-muted-foreground" aria-label={row.kindLabel} />
      <div className="min-w-0 flex-1">
        <div className="truncate text-[13px] font-medium">{row.title}</div>
        {row.subtitle ? <div className="truncate text-xs text-muted-foreground">{row.subtitle}</div> : null}
      </div>
      <span className="shrink-0 text-xs text-muted-foreground">{row.updated}</span>
      <LockButton
        state={row.locked ? "locked" : "unlocked"}
        name={row.title}
        unlockIds={row.locked && !row.identityProvider ? [row.id] : []}
        lockIds={row.locked ? [] : [row.id]}
        help={row.lockHelp}
        actions={actions}
        blocked={blocked}
      />
    </div>
  );
}

/** The lock: closed while every use needs approval, open once unattended
 * access is allowed. A click flips it for what it stands for (an item, a
 * site, an app); unlocking asks first, then Touch ID once. */
function LockButton({ state, name, unlockIds, lockIds, help, actions, blocked }: LockSpec & { help?: string; actions: LockActions; blocked: boolean }) {
  const Icon = state === "locked" ? LockIcon : LockOpenIcon;
  const text =
    help ||
    (state === "unlocked" ? "Unlocked. Click to lock." : state === "mixed" ? "Some unlocked. Click to unlock the rest." : "Locked. Click to allow unattended access.");
  return (
    <button
      type="button"
      data-lock={state}
      aria-label={state === "unlocked" ? `Lock ${name}` : `Unlock ${name}`}
      title={text}
      disabled={blocked || (state === "locked" && unlockIds.length === 0)}
      onClick={(e) => {
        e.stopPropagation();
        if (state === "unlocked") actions.lock(lockIds);
        else if (unlockIds.length) actions.unlock(unlockIds, unlockIds.length === 1 ? name : undefined);
      }}
      className={cn(
        "flex size-6 shrink-0 items-center justify-center rounded-md outline-none hover:bg-foreground/[0.06] focus-visible:ring-2 focus-visible:ring-ring/60 disabled:opacity-40",
        state === "locked" ? "text-muted-foreground" : "text-amber-600 dark:text-amber-400",
        state === "mixed" && "opacity-60",
      )}
    >
      <Icon className="size-3.5" />
    </button>
  );
}

/** What the selection can do, together: lock, unlock, delete. */
function BatchBar({
  selection,
  blocked,
  busy,
  send,
  actions,
  onDelete,
}: {
  selection: VaultSelection;
  blocked: boolean;
  busy: boolean;
  send: (a: VaultAction) => void;
  actions: LockActions;
  onDelete: (ids: string[]) => void;
}) {
  return (
    <div className="flex shrink-0 items-center gap-2 border-t bg-muted/60 px-8 py-2.5" data-vault-batch>
      <span className="text-[13px] font-medium">{selection.title}</span>
      <Button size="sm" variant="ghost" onClick={() => send({ type: "clear" })}>
        Clear
      </Button>
      <span className="flex-1" />
      <Button size="sm" variant="outline" disabled={!selection.canLock || busy} onClick={() => actions.lock(selection.lockIds)}>
        <LockIcon /> Lock
      </Button>
      <Button
        size="sm"
        variant="outline"
        disabled={!selection.canUnlock || blocked || busy}
        title={selection.alwaysAsk > 0 ? "Identity provider sessions always ask and stay locked." : undefined}
        onClick={() => actions.unlock(selection.unlockIds)}
      >
        <LockOpenIcon /> Unlock
      </Button>
      <Button size="sm" variant="outline" disabled={busy} onClick={() => onDelete(selection.ids)}>
        <Trash2Icon /> Delete
      </Button>
    </div>
  );
}
