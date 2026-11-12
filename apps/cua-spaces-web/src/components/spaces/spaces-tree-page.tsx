// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link, useNavigate } from "@tanstack/react-router";
import { ChevronRightIcon, HardDriveIcon, MoreHorizontalIcon, PlusIcon, TriangleAlertIcon } from "lucide-react";
import { useEffect, useRef, useState, type ReactNode } from "react";

import { listView, useBridge, useMachines, useNotchPins, useResources, useSpaces, useSpacesDefault, type CreatingElsewhere, type SpaceOs } from "@/bridge";
import { AddComputerSheet } from "@/components/add-computer-sheet";
import { NewSpaceButton } from "@/components/new-space/new-space-button";
import { computerWarnings } from "@/components/resources/computer-resources";
import { EnrollSheet, useMachinesAccount, usePendingActions } from "@/components/machines/account-parts";
import { PageTip } from "@/components/tips";
import { UpsellPrompt } from "@/components/upsell-prompt";
import { Page, PageHeader } from "@/components/page";
import { Button } from "@/components/ui/button";
import { ConfirmDialog } from "@/components/ui/alert-dialog";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { GroupedSection } from "@/components/ui/grouped";
import { Input } from "@/components/ui/input";
import { Menu, type MenuItem } from "@/components/ui/menu";
import { toast, toastError } from "@/components/ui/toast";
import { useNewSpace } from "@/hooks/use-new-space";
import { t, useT } from "@/i18n";
import { THIS_COMPUTER, computerParam } from "@/lib/machines";
import type { ComputersSearch } from "@/lib/old-links";
import { descendants, foldOffline, treeSummary, type TreeNode } from "@/lib/spaces-tree";
import { createProgress, spaceState } from "@/lib/spaces";
import { cn } from "@/lib/utils";

import { PinnedMark } from "./space-tile";
import { CloudsSection, DevicesSection, PendingSection, RemoveOldButton } from "./tree-extras";
import { PeopleFaces, TreeDot, TreeIcon, toggleCollapsed, useCollapsed } from "./tree-parts";
import { useRunningBySpace, useSpacesTreeData } from "./use-spaces-tree";
import { thisComputerLabel, thisComputerText } from "@/lib/host-labels";

// Name gets the most room; Status shares what is left, so neither is cut to a few letters in a narrow window.
/** How long a click on a name waits for a second one (a double-click renames) before it opens the row. */
export const DOUBLE_CLICK_MS = 300;

const COLS = "grid grid-cols-[minmax(0,3fr)_minmax(0,2fr)_96px_96px_32px] items-center gap-3";

/** "Safari check, 1 agent task stops". */
export function stopsLine(node: Pick<TreeNode, "name" | "running">): string {
  if (node.running === 0) return node.name;
  return t("computers.tree.stops_line", { name: node.name, count: node.running });
}

/** The remove dialog's words (f9): what leaves the list, what stops, and that nothing is deleted unless asked. */
export function removeText(
  node: Pick<TreeNode, "name" | "children">,
  alsoDelete = false,
): { title: string; lead: string; keep: string; box: string | null; confirm: string } {
  const n = descendants(node).length;
  const lead = n === 0 ? t("computers.tree.remove_lead_none") : t("computers.tree.remove_lead_spaces", { count: n });
  const keepKey = n === 0 ? "computers.tree.remove_keep_none" : n === 1 ? "computers.tree.remove_keep_one" : "computers.tree.remove_keep_many";
  // A computer is removed while it is offline (CUA-3479): ticked, its Spaces leave the account now, and the relay keeps
  // them as pending deletions that the computer carries out, files included, the next time it comes online.
  const deleted =
    n === 1 ? t("computers.tree.remove_deleted_one", { name: node.name }) : t("computers.tree.remove_deleted_many", { name: node.name, count: n });
  return {
    title: t("computers.tree.remove_title", { name: node.name }),
    lead,
    keep: alsoDelete ? deleted : t(keepKey, { name: node.name, count: n }),
    box: n === 0 ? null : t("computers.tree.remove_also_delete", { count: n }),
    confirm: alsoDelete ? t("computers.tree.remove_and_delete") : t("computers.tree.remove_named", { name: node.name }),
  };
}

/**
 * Your computers as one list (CUA-3413, CUA-3551): each computer you added,
 * the Spaces made on it under it, and for every row its status, who's on it
 * and its running agent tasks. A computer opens its own page, a Space its
 * own. Around the list: computers waiting to be added, the clouds you
 * connected and the other devices signed in to your account.
 */
export function SpacesTreePage({ search = {}, onSearchDone }: { search?: ComputersSearch; onSearchDone?: () => void }) {
  const t = useT();
  const running = useRunningBySpace();
  const { tree, loaded, defaults } = useSpacesTreeData(running);
  const { data: spaces = [], listStatus, renameSpace, deleteSpace, startSpace, stopSpace, creatingElsewhere } = useSpaces();
  const { core } = useBridge();
  const machines = useMachines();
  const resources = useResources();
  const pins = useNotchPins();
  const navigate = useNavigate();
  const newSpace = useNewSpace();
  const collapsed = useCollapsed();
  const { pending, account, devices } = useMachinesAccount();
  const pendingActions = usePendingActions();
  const asked = search.add === true || search.join !== undefined;
  const [adding, setAddingState] = useState(asked);
  const setAdding = (open: boolean) => {
    setAddingState(open);
    if (!open && asked) onSearchDone?.();
  };
  useEffect(() => {
    if (asked) setAddingState(true);
  }, [asked]);
  const [editing, setEditing] = useState<{ key: string; name: string } | null>(null);
  const [removing, setRemoving] = useState<TreeNode | null>(null);
  const [deleting, setDeleting] = useState<TreeNode | null>(null);
  // Offline computers with nothing on them sit folded in one "N offline" row until it's opened.
  const [showOffline, setShowOffline] = useState(false);
  // A click on a name waits out the double-click time before opening (see clickName below).
  const clickTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cancelClick = () => {
    if (clickTimer.current) clearTimeout(clickTimer.current);
    clickTimer.current = null;
  };
  useEffect(() => cancelClick, []);

  // The core decides: the empty list only for a complete list with nothing on it. A daemon still starting (an app update)
  // or computers still answering show what is known and one quiet line instead.
  const listed = listView(core, listStatus, spaces.length);
  // The "Waiting for you" section sits above the list, so hold the page until the account answered: a late
  // answer would push the whole list down (layout shift, CUA-1951).
  if (!tree || !loaded || devices.isLoading)
    return (
      <Page>
        <div aria-busy="true" aria-label={t("computers.page.loading")}>
          {listed.reachingLine ? (
            <p className="text-[13px] text-muted-foreground" data-spaces-reaching="">
              {listed.reachingLine}
            </p>
          ) : null}
        </div>
      </Page>
    );

  const addButton = (
    <>
      <Button variant="outline" onClick={() => setAdding(true)} data-tree-add-computer="">
        <PlusIcon /> {t("computers.page.add_computer")}
      </Button>
      <NewSpaceButton />
    </>
  );
  const sheets = (
    <>
      <AddComputerSheet
        open={adding}
        onOpenChange={setAdding}
        initialView={search.join !== undefined ? "enter" : "pick"}
        initialCode={search.join ?? ""}
        initialRelay={search.relay ?? null}
      />
      <EnrollSheet />
      {pendingActions.notMeDialog((machine) => machines.remove(machine, null))}
    </>
  );
  const waiting = <PendingSection pending={pending} title={account?.pendingTitle ?? t("computers.page.waiting")} actions={pendingActions} />;

  // f6: nothing yet. Agents work on a computer you own; a new Space on this Mac is the second choice.
  // The sentence is one string with the button's place marked; the button sits where the mark was.
  const [newLocalBefore = "", newLocalAfter = ""] = t("computers.page.empty_new_local", { action: "\u0001" }).split("\u0001");
  if (tree.roots.length === 0 && creatingElsewhere.length === 0 && listed.empty) {
    return (
      <Page>
        {waiting}
        <div className="mx-auto max-w-xl pt-24" data-tree-empty="">
          <h1 className="font-heading text-[32px] font-semibold tracking-tight">{t("computers.page.empty_title")}</h1>
          <p className="mt-3 text-[15px] text-muted-foreground">{t("computers.page.empty_body", { thisComputer: thisComputerText() })}</p>
          <div className="mt-6 flex flex-wrap gap-2">
            <Button size="lg" render={<Link to="/spaces/computer/$computerId" params={{ computerId: THIS_COMPUTER }} />} data-tree-add-this-mac="">
              {t("computers.page.empty_add_this", { thisComputer: thisComputerText() })}
            </Button>
            <Button size="lg" variant="outline" onClick={() => setAdding(true)}>
              {t("computers.page.empty_add_other")}
            </Button>
          </div>
          <p className="mt-5 text-[13px] text-muted-foreground">
            {newLocalBefore}
            <button type="button" className="underline underline-offset-2 hover:text-foreground" onClick={newSpace} data-tree-new-local="">
              {t("computers.page.empty_new_local_action", { thisComputer: thisComputerText() })}
            </button>
            {newLocalAfter}
          </p>
        </div>
        {sheets}
      </Page>
    );
  }

  // A computer opens its own page (its switches, Spaces, details and Remove), a Space its own.
  const open = (n: TreeNode) =>
    n.kind === "computer" && n.opens
      ? void navigate({ to: "/spaces/$spaceId", params: { spaceId: n.opens } })
      : n.kind === "computer" && n.machine
        ? void navigate({ to: "/spaces/computer/$computerId", params: { computerId: computerParam(n.machine) } })
        : n.space
          ? void navigate({ to: "/spaces/$spaceId", params: { spaceId: n.space.id } })
          : undefined;
  // A click on a name opens it, a double-click renames it. A real double-click starts with a click, so a click waits out
  // the double-click time before opening, and a double-click cancels it. A keyboard press (detail 0) opens at once.
  const clickName = (n: TreeNode, detail: number) => {
    cancelClick();
    if (detail === 0 || !canRename(n)) return open(n);
    if (detail > 1) return;
    clickTimer.current = setTimeout(() => {
      clickTimer.current = null;
      open(n);
    }, DOUBLE_CLICK_MS);
  };
  const canRename = (n: TreeNode) => (n.kind === "computer" ? Boolean(n.machine) : n.space?.access?.canRename !== false);
  const saveRename = (n: TreeNode, raw: string) => {
    setEditing(null);
    const name = raw.trim();
    if (!name || name === n.name) return;
    const done = n.kind === "computer" ? machines.rename(n.current ? "this-mac" : n.machine!.id, name) : renameSpace(n.space!.id, name);
    void done.then(() => toast(t("computers.page.renamed", { name }), { type: "success" }), toastError(t("computers.page.rename_failed", { name: n.name })));
  };
  // A Space, or another computer's shared desktop, can be pinned to the notch; This Mac's own desktop never is.
  const pinnable = (n: TreeNode) => pins.canPin && Boolean(n.space) && !n.current;
  const viewRemove = (n: TreeNode) => machines.view?.machines.find((m) => m.id === n.machine?.id)?.remove ?? null;
  const menu = (n: TreeNode): MenuItem[] => {
    const items: MenuItem[] = [{ label: t("computers.page.open"), onSelect: () => open(n) }];
    // A computer whose row opens its desktop keeps its own page (switches, details, Remove) one step away.
    if (n.kind === "computer" && n.opens && n.machine)
      items.push({
        label: t("computers.page.computer_settings"),
        onSelect: () => void navigate({ to: "/spaces/computer/$computerId", params: { computerId: computerParam(n.machine!) } }),
      });
    if (canRename(n)) items.push({ label: t("computers.page.rename"), onSelect: () => setEditing({ key: n.key, name: n.name }) });
    if (n.settingsId && !n.isDefault && !defaults.unsupported)
      items.push({
        label: t("computers.page.make_default"),
        onSelect: () => void defaults.makeDefault(n.settingsId!).catch(toastError(t("computers.page.default_failed"))),
      });
    if (n.kind === "computer" && !n.current && n.machine) {
      const r = viewRemove(n);
      // The core keeps Remove off while a computer is online. The tree's offline (the relay's own answer) wins over a
      // machines list that lags; the relay still refuses a computer that is connected.
      items.push({
        label: t("computers.tree.remove_named", { name: n.name }),
        destructive: true,
        disabled: r ? !r.enabled && n.online : false,
        onSelect: () => setRemoving(n),
      });
    }
    // Turn a Space on or off from its row, as the Space page's power button does (a Space that can be turned off, by its true
    // state; one being made, failed or not yours has none). Turning a parent off turns its child Spaces off with it.
    if (n.kind === "space" && n.space?.power && n.space.access?.canDelete !== false) {
      const state = spaceState(n.space);
      const id = n.space.id;
      if (state === "running") {
        items.push({
          label: n.children.length > 0 ? t("computers.page.turn_off_with_children") : t("computers.page.turn_off"),
          onSelect: () => void stopSpace(id).catch(toastError(t("computers.page.turn_off_failed", { name: n.name }))),
        });
      } else if (state === "stopped") {
        items.push({
          label: t("computers.page.turn_on"),
          onSelect: () => void startSpace(id).catch(toastError(t("computers.page.turn_on_failed", { name: n.name }))),
        });
      }
    }
    if (pinnable(n)) {
      const id = n.space!.id;
      items.push({
        label: pins.pinned.has(id) ? t("computers.page.unpin") : t("computers.page.pin"),
        onSelect: () => void pins.toggle(id).catch(toastError(t("computers.menu.notch_failed"))),
      });
    }
    if (n.kind === "space" && n.space?.access?.canDelete !== false)
      items.push({ label: t("computers.page.delete"), destructive: true, onSelect: () => setDeleting(n) });
    return items;
  };

  // A computer with a low disk says so on its own row (a small mark by its status), not in a banner above the list.
  const diskMarks = new Map(
    computerWarnings(resources, machines.data ?? []).map(
      (w) =>
        [w.machineId, { level: w.level, title: t(w.level === "critical" ? "computers.row.disk_full" : "computers.row.disk_low", { free: w.free }) }] as const,
    ),
  );
  const rowFor = (n: TreeNode, depth: number) => {
    const folded = collapsed.has(n.key);
    const state = n.space ? spaceState(n.space) : null;
    const isEditing = editing?.key === n.key;
    const warning = n.kind === "computer" && n.machine ? diskMarks.get(n.machine.id) : undefined;
    return (
      <div
        key={n.key}
        role="row"
        data-tree-row={n.key}
        data-tree-kind={n.kind}
        data-dim={n.dim ? "" : undefined}
        // The core's rows, for the parity flows: a computer's machine, and the Space a row opens.
        data-machine-id={n.kind === "computer" && n.machine ? n.machine.id : undefined}
        data-machine-name={n.kind === "computer" && n.machine ? n.machine.name : undefined}
        data-online={n.kind === "computer" && n.machine ? String(n.online) : undefined}
        data-space-id={n.space?.id}
        data-state={state ?? undefined}
        className={cn(COLS, "min-h-12 px-4 py-1.5 text-[14px]", n.dim && "text-muted-foreground")}
      >
        <div role="cell" className={cn("flex min-w-0 items-center gap-1.5", isEditing && "col-span-2")} style={{ paddingLeft: depth * INDENT_PX }}>
          {/* One slot for the fold caret on every row (empty when there is nothing to fold), inside the row's padding: the
              caret never hangs out of the table's edge, and the icons line up at every depth. */}
          <span className="flex size-5 shrink-0 items-center justify-center">
            {n.children.length > 0 ? (
              <button
                type="button"
                aria-label={folded ? t("computers.page.show_spaces", { name: n.name }) : t("computers.page.hide_spaces", { name: n.name })}
                aria-expanded={!folded}
                onClick={() => toggleCollapsed(n.key)}
                className="flex size-5 items-center justify-center rounded-lg text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/60"
                data-tree-fold={n.key}
              >
                <ChevronRightIcon className={cn("size-3.5 transition-transform", !folded && "rotate-90")} />
              </button>
            ) : null}
          </span>
          <TreeIcon node={n} className={cn(n.dim && "opacity-50")} />
          {isEditing ? (
            <Input
              autoFocus
              aria-label={t("computers.page.new_name", { name: n.name })}
              value={editing.name}
              maxLength={64}
              onChange={(e) => setEditing({ key: n.key, name: e.currentTarget.value })}
              onKeyDown={(e) => {
                if (e.key === "Enter") saveRename(n, editing.name);
                if (e.key === "Escape") setEditing(null);
              }}
              onBlur={() => setEditing(null)}
              data-tree-rename-input=""
              className="h-8 w-full min-w-0 text-[14px]"
            />
          ) : (
            <button
              type="button"
              onClick={(e) => clickName(n, e.detail)}
              onDoubleClick={(e) => {
                e.preventDefault();
                cancelClick();
                if (canRename(n)) setEditing({ key: n.key, name: n.name });
              }}
              className={cn("min-w-0 truncate text-left outline-none hover:underline focus-visible:underline", n.kind === "computer" && "font-semibold")}
              data-tree-name={n.key}
              data-space-name={n.kind === "space" ? "" : undefined}
            >
              {n.name}
            </button>
          )}
          {/* Folded, a Space says how many child Spaces are under it. */}
          {n.kind === "space" && folded && n.children.length > 0 && !isEditing ? (
            <span data-tree-folded-count={n.key} className="shrink-0 text-xs text-muted-foreground">
              {t("computers.page.child_count", { count: descendants(n).length })}
            </span>
          ) : null}
          {pinnable(n) && !isEditing ? <PinnedMark id={n.space!.id} /> : null}
        </div>
        {isEditing ? (
          <div role="cell" className="col-span-3 text-right text-[13px] text-muted-foreground">
            {t("computers.page.rename_hint")}
          </div>
        ) : (
          <>
            <div role="cell" className="flex min-w-0 items-center gap-2 text-[13px] text-muted-foreground" data-tree-status={n.key}>
              <TreeDot tone={n.tone} />
              {/* A Space being made says its step ("Downloading macOS (first time only)…") and how far it got, as the
                  sidebar's Making card does. */}
              <span className="truncate">{state === "creating" && n.space?.progress?.label ? n.space.progress.label : n.status}</span>
              {warning ? (
                // A low disk is a mark on the computer's own row; its page says the rest.
                <span title={warning.title} data-tree-disk-warning={warning.level} className="flex shrink-0 text-warning">
                  <TriangleAlertIcon aria-hidden className="size-3.5" />
                  <span className="sr-only">{warning.title}</span>
                </span>
              ) : null}
              {state === "creating" && n.space ? (
                <span className="h-1 w-12 shrink-0 overflow-hidden rounded-full bg-foreground/10" aria-hidden>
                  <span
                    data-create-progress=""
                    className="block h-full rounded-full bg-brand"
                    style={{ width: `${Math.round(createProgress(n.space) * 100)}%` }}
                  />
                </span>
              ) : null}
              {state === "creating" && n.space?.progress?.transfer ? (
                <span className="truncate" data-create-transfer="">
                  {n.space.progress.transfer}
                </span>
              ) : null}
            </div>
            <div role="cell">
              <PeopleFaces people={n.people} />
            </div>
            <div role="cell" className="text-[13px] text-muted-foreground" data-tree-tasks={n.key}>
              {n.tasks}
            </div>
            <div role="cell" className="flex justify-end">
              <Menu
                items={menu(n)}
                trigger={
                  <button
                    type="button"
                    aria-label={t("computers.menu.more_for", { name: n.name })}
                    className="flex size-7 items-center justify-center rounded-lg text-muted-foreground hover:bg-foreground/5 hover:text-foreground"
                    data-tree-menu={n.key}
                  >
                    <MoreHorizontalIcon className="size-4" />
                  </button>
                }
              />
            </div>
          </>
        )}
      </div>
    );
  };

  // This Mac before it is set up, with no Space on it, is no computer of the list yet; its row still opens its page,
  // where it's set up and where its settings (Cua Driver) live.
  const here = (machines.data ?? []).find((m) => m.current && !m.device);
  const hereRow =
    here && !tree.roots.some((r) => r.current) ? (
      <div
        key="this-mac-not-set-up"
        role="row"
        data-tree-row="this-mac"
        data-tree-kind="computer"
        data-tree-not-set-up=""
        data-machine-id={here.id}
        data-machine-name={here.name}
        data-online={String(here.online)}
        className={cn(COLS, "min-h-12 px-4 py-1.5 text-[14px]")}
      >
        <div role="cell" className="flex min-w-0 items-center gap-1.5">
          <span className="size-5 shrink-0" aria-hidden />
          <TreeIcon node={{ kind: "computer", current: true, os: null }} />
          <Link
            to="/spaces/computer/$computerId"
            params={{ computerId: THIS_COMPUTER }}
            className="min-w-0 truncate font-semibold outline-none hover:underline focus-visible:underline"
            data-tree-name="this-mac"
          >
            {thisComputerLabel()}
          </Link>
        </div>
        <div role="cell" className="flex min-w-0 items-center gap-2 text-[13px] text-muted-foreground" data-tree-status="this-mac">
          <TreeDot tone="offline" />
          <span className="truncate">{t("computers.page.not_set_up")}</span>
        </div>
        <div role="cell" />
        <div role="cell" />
        <div role="cell" className="flex justify-end">
          <Menu
            items={[
              { label: t("computers.page.open"), onSelect: () => void navigate({ to: "/spaces/computer/$computerId", params: { computerId: THIS_COMPUTER } }) },
            ]}
            trigger={
              <button
                type="button"
                aria-label={t("computers.menu.more_for", { name: thisComputerLabel() })}
                className="flex size-7 items-center justify-center rounded-lg text-muted-foreground hover:bg-foreground/5 hover:text-foreground"
                data-tree-menu="this-mac"
              >
                <MoreHorizontalIcon className="size-4" />
              </button>
            }
          />
        </div>
      </div>
    ) : null;
  const single = tree.computers === 1 && tree.spaces === 0;
  const { shown, folded } = foldOffline(tree.roots);
  // Spaces another of your computers is making (CUA-2349) sit under it until they are listed; any whose computer
  // isn't in the list come last.
  const elsewhereOn = (r: TreeNode) => creatingElsewhere.filter((c) => r.machine && c.machineId === r.machine.id);
  const strays = creatingElsewhere.filter((c) => !tree.roots.some((r) => r.machine?.id === c.machineId));
  // A row, then (unless folded) the Spaces under it; a Space's child Spaces sit under it one step further in.
  const rows = (n: TreeNode, depth: number): ReactNode[] => [rowFor(n, depth), ...(collapsed.has(n.key) ? [] : n.children.flatMap((c) => rows(c, depth + 1)))];
  const branch = (r: TreeNode) => [
    rowFor(r, 0),
    ...(collapsed.has(r.key)
      ? []
      : [
          ...r.children.map((c) => rowFor(c, 1)),
          ...elsewhereOn(r).map((c) => <ElsewhereRow key={`elsewhere:${c.machineId}/${c.name}`} creating={c} nested />),
        ]),
  ];
  const offlineRow =
    folded.length > 0 ? (
      <div key="offline" role="row" data-tree-offline-fold="" className={cn(COLS, "min-h-12 px-4 py-1.5 text-[14px] text-muted-foreground")}>
        {/* One quiet sentence, level with the names above it (past the caret slot and the icon). */}
        <div role="cell" className="col-span-5 flex min-w-0 items-center gap-3" style={{ paddingLeft: INDENT_PX }}>
          <span className="min-w-0 truncate">
            {t(showOffline ? "computers.page.offline_shown" : "computers.page.offline_hidden", { count: folded.length })}
            {"\u00a0"}
            <button
              type="button"
              aria-expanded={showOffline}
              onClick={() => setShowOffline((v) => !v)}
              className="cursor-default underline decoration-border underline-offset-2 outline-none hover:text-foreground focus-visible:text-foreground"
              data-tree-offline-toggle=""
            >
              {showOffline ? t("computers.page.offline_hide") : t("computers.page.offline_show")}
            </button>
          </span>
          {showOffline ? <RemoveOldButton /> : null}
        </div>
      </div>
    ) : null;
  return (
    <Page>
      <PageHeader title={t("computers.page.title")} description={editing ? t("computers.page.rename_help") : treeSummary(tree)} actions={addButton} />
      <PageTip place="spaces" />
      {waiting}
      <div role="table" aria-label={t("computers.page.table")} data-spaces-tree="">
        <div role="row" className={cn(COLS, "px-4 pb-2 text-[13px] text-muted-foreground")}>
          <div role="columnheader" className="pl-[26px]">
            {t("computers.page.col_name")}
          </div>
          <div role="columnheader">{t("computers.page.col_status")}</div>
          <div role="columnheader">{t("computers.page.col_people")}</div>
          <div role="columnheader">{t("computers.page.col_tasks")}</div>
          <div role="columnheader" aria-label={t("computers.page.col_actions")} />
        </div>
        <GroupedSection role="rowgroup">
          {hereRow}
          {shown.flatMap(branch)}
          {offlineRow}
          {showOffline ? folded.map((r) => rowFor(r, 0)) : null}
          {strays.map((c) => (
            <ElsewhereRow key={`elsewhere:${c.machineId}/${c.name}`} creating={c} />
          ))}
        </GroupedSection>
      </div>
      {listed.reachingLine ? (
        <p className="mt-3 px-4 text-[13px] text-muted-foreground" data-spaces-reaching="">
          {listed.reachingLine}
        </p>
      ) : null}
      {single ? (
        // f7: only This Mac. Offer to add another computer they own.
        <GroupedSection className="mt-8" data-tree-single="">
          <div className="flex items-center gap-4 px-5 py-4">
            <HardDriveIcon className="size-6 shrink-0 text-muted-foreground" strokeWidth={1.5} />
            <div className="min-w-0 flex-1">
              <div className="text-[15px] font-medium">{t("computers.page.single_title")}</div>
              <div className="text-[13px] text-muted-foreground">{t("computers.page.single_body")}</div>
            </div>
            <Button variant="outline" onClick={() => setAdding(true)}>
              {t("computers.page.add_computer")}
            </Button>
          </div>
        </GroupedSection>
      ) : null}
      <UpsellPrompt surface="machine" active={(machines.data ?? []).length >= 2 && (machines.data ?? []).some((m) => m.via === "relay")} className="mt-4" />
      <CloudsSection />
      <DevicesSection />
      {sheets}
      {removing ? <RemoveComputerDialog node={removing} onClose={() => setRemoving(null)} /> : null}
      <ConfirmDialog
        open={deleting !== null}
        onOpenChange={(o) => !o && setDeleting(null)}
        title={deleting ? t("computers.page.delete_title", { name: deleting.name }) : ""}
        description={
          deleting && deleting.children.length > 0
            ? t("computers.page.delete_body_children", {
                count: descendants(deleting).length,
                names: namesOf(descendants(deleting).map((d) => d.name)),
              })
            : t("computers.page.delete_body")
        }
        confirmLabel={t("computers.page.delete")}
        destructive
        cancelFirst
        onConfirm={() => {
          const n = deleting!;
          setDeleting(null);
          void deleteSpace(n.space!.id).then(
            () => toast(t("computers.page.deleted", { name: n.name }), { type: "success" }),
            toastError(t("computers.page.delete_failed", { name: n.name })),
          );
        }}
      />
    </Page>
  );
}

/** A Space another computer is making: its name, Getting ready and how far it got. Nothing to open yet. */
/** "kid-a", "kid-a and kid-b", "kid-a, kid-b and kid-c" (names only up to four, then a count). */
export function namesOf(names: readonly string[]): string {
  if (names.length === 0) return "";
  if (names.length > 4) return t("computers.page.names_more", { list: names.slice(0, 3).join(", "), count: names.length - 3 });
  if (names.length === 1) return names[0]!;
  return t("computers.page.names_and", { list: names.slice(0, -1).join(", "), last: names[names.length - 1]! });
}
/** How far each level of child Spaces sits in from its parent (px): one caret slot and its gap. */
const INDENT_PX = 26;

function ElsewhereRow({ creating: c, nested }: { creating: CreatingElsewhere; nested?: boolean }) {
  const t = useT();
  const os: SpaceOs = c.os === "macos" || c.os === "linux" || c.os === "windows" ? c.os : "unknown";
  const percent = `${Math.min(100, Math.max(0, Math.round(c.percent)))}%`;
  return (
    <div
      role="row"
      data-creating-elsewhere={c.machineId}
      aria-label={t("computers.page.elsewhere_label", { name: c.name, machine: c.machineName.trim() || t("computers.page.another_computer"), percent })}
      className={cn(COLS, "min-h-12 px-4 py-1.5 text-[14px]")}
    >
      <div role="cell" className="flex min-w-0 items-center gap-1.5" style={{ paddingLeft: nested ? INDENT_PX : 0 }}>
        <span className="size-5 shrink-0" aria-hidden />
        <TreeIcon node={{ kind: "space", current: false, os }} />
        <span className="min-w-0 truncate">{c.name}</span>
      </div>
      <div role="cell" className="flex min-w-0 items-center gap-2 text-[13px] text-muted-foreground">
        <TreeDot tone="busy" />
        <span className="truncate">{t("computers.tree.space_creating")}</span>
        <span className="h-1 w-12 shrink-0 overflow-hidden rounded-full bg-foreground/10" aria-hidden>
          <span className="block h-full rounded-full bg-brand" style={{ width: percent }} />
        </span>
        <span className="tabular-nums" data-state-label="">
          {percent}
        </span>
      </div>
      <div role="cell" />
      <div role="cell" />
      <div role="cell" />
    </div>
  );
}

/**
 * f9: removing a computer names each Space made on it and any task that
 * stops. They are only detached unless "Also delete" is ticked: then the
 * relay forgets them at once and the computer deletes them, files included,
 * the next time it comes online (pending deletions, CUA-3479).
 */
export function RemoveComputerDialog({ node, onClose }: { node: TreeNode; onClose: () => void }) {
  const t = useT();
  const machines = useMachines();
  const defaults = useSpacesDefault();
  const [alsoDelete, setAlsoDelete] = useState(false);
  const [busy, setBusy] = useState(false);
  const text = removeText(node, alsoDelete);
  const r = machines.view?.machines.find((m) => m.id === node.machine?.id)?.remove ?? null;
  const run = async () => {
    setBusy(true);
    try {
      await machines.remove(r?.machine ?? node.machine!.id, r?.device ?? null, alsoDelete);
      // The default setting may have named it (the host clears it): read it again, so the tree has no ghost of it.
      await defaults.refresh().catch(() => {});
      toast(t("computers.page.removed", { name: node.name }), { type: "success" });
      onClose();
    } catch (e) {
      toastError(t("computers.page.remove_failed", { name: node.name }))(e);
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog open onOpenChange={(o) => !o && !busy && onClose()}>
      <DialogPopup className="w-[min(480px,calc(100vw-2rem))]" data-tree-remove={node.key}>
        <div className="px-6 pt-6 pb-5">
          <DialogTitle className="text-[17px] font-semibold">{text.title}</DialogTitle>
          <p className="mt-3 text-[14px] text-muted-foreground">{text.lead}</p>
          {node.children.length > 0 ? (
            <ul className="mt-3 flex flex-col gap-1.5 text-[14px]" data-tree-remove-children="">
              {descendants(node).map((c) => (
                <li key={c.key} className="flex items-center gap-2.5">
                  <TreeIcon node={c} />
                  {stopsLine(c)}
                </li>
              ))}
            </ul>
          ) : null}
          <p className="mt-3 text-[14px] text-muted-foreground">{text.keep}</p>
          {text.box ? (
            <label className="mt-4 flex items-start gap-2.5 text-[14px]">
              <Checkbox checked={alsoDelete} onCheckedChange={(v) => setAlsoDelete(v === true)} data-tree-also-delete="" />
              <span>{text.box}</span>
            </label>
          ) : null}
        </div>
        {/* design-ok: a dialog's footer bar, as ConfirmDialog's */}
        <div className="flex justify-end gap-2 border-t bg-muted/60 px-6 py-3">
          <Button variant="outline" disabled={busy} onClick={onClose}>
            {t("computers.page.cancel")}
          </Button>
          <Button variant="destructive" disabled={busy} onClick={() => void run()} data-tree-remove-confirm="">
            {text.confirm}
          </Button>
        </div>
      </DialogPopup>
    </Dialog>
  );
}
