// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Link, useNavigate } from "@tanstack/react-router";
import { ChevronRightIcon, HardDriveIcon, MoreHorizontalIcon, PlusIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { useMachines, useSpaces } from "@/bridge";
import { AddComputerSheet } from "@/components/add-computer-sheet";
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
import { plural } from "@/lib/plural";
import { treeSummary, type TreeNode } from "@/lib/spaces-tree";
import { cn } from "@/lib/utils";

import { PeopleFaces, TreeDot, TreeIcon, toggleCollapsed, useCollapsed } from "./tree-parts";
import { useRunningBySpace, useSpacesTreeData } from "./use-spaces-tree";

// Name gets the most room; Status shares what is left, so neither is cut to a few letters in a narrow window.
/** How long a click on a name waits for a second one (a double-click renames) before it opens the row. */
export const DOUBLE_CLICK_MS = 300;

const COLS = "grid grid-cols-[minmax(0,3fr)_minmax(0,2fr)_96px_96px_32px] items-center gap-3";

export const RENAME_NOTE = "Renaming changes the name only. Your team sees the new name, and agent tasks and links keep working.";

/** "Safari check, 1 agent task stops". */
export function stopsLine(node: Pick<TreeNode, "name" | "running">): string {
  if (node.running === 0) return node.name;
  return `${node.name}, ${plural(node.running, "agent task")} ${node.running === 1 ? "stops" : "stop"}`;
}

/** The remove dialog's words (f9): what leaves the list, what stops, and that nothing is deleted unless asked. */
export function removeText(node: Pick<TreeNode, "name" | "children">): { title: string; lead: string; keep: string; box: string | null; confirm: string } {
  const n = node.children.length;
  const lead =
    n === 0
      ? "It leaves your Spaces list, and your team can't use it any more."
      : `It leaves your Spaces list, and your team can't use it any more. The ${plural(n, "Space")} made on it leave${n === 1 ? "s" : ""} the list with it:`;
  const back = n === 0 ? "" : n === 1 ? " Add it again and it comes back." : ` Add it again and all ${n} come back.`;
  return {
    title: `Remove ${node.name}?`,
    lead,
    keep: `Nothing is deleted from ${node.name}.${back}`,
    box: n === 0 ? null : `Also delete the ${n === 1 ? "Space" : `${n} Spaces`} and ${n === 1 ? "its" : "their"} files from ${node.name}`,
    confirm: `Remove ${node.name}`,
  };
}

/**
 * The Spaces page as one list (Labs "Spaces as a tree", CUA-3413): each
 * computer you added, the Spaces made on it under it, and for every row its
 * status, who's on it and its running agent tasks.
 */
export function SpacesTreePage() {
  const running = useRunningBySpace();
  const { tree, loaded, defaults } = useSpacesTreeData(running);
  const { renameSpace, deleteSpace } = useSpaces();
  const machines = useMachines();
  const navigate = useNavigate();
  const newSpace = useNewSpace();
  const collapsed = useCollapsed();
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<{ key: string; name: string } | null>(null);
  const [removing, setRemoving] = useState<TreeNode | null>(null);
  const [deleting, setDeleting] = useState<TreeNode | null>(null);
  // A click on a name waits out the double-click time before opening (see clickName below).
  const clickTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const cancelClick = () => {
    if (clickTimer.current) clearTimeout(clickTimer.current);
    clickTimer.current = null;
  };
  useEffect(() => cancelClick, []);

  if (!tree || !loaded) return <Page><div aria-busy="true" aria-label="Loading Spaces" /></Page>;

  const addButton = (
    <Button onClick={() => setAdding(true)} data-tree-add-computer="">
      <PlusIcon /> Add a computer
    </Button>
  );
  const sheet = <AddComputerSheet open={adding} onOpenChange={setAdding} />;

  // f6: nothing yet. A Space is a computer you own; a new one on this Mac is the second choice.
  if (tree.roots.length === 0) {
    return (
      <Page>
        <div className="mx-auto max-w-xl pt-24" data-tree-empty="">
          <h1 className="font-heading text-[32px] font-semibold tracking-tight">No Spaces yet</h1>
          <p className="mt-3 text-[15px] text-muted-foreground">A Space is a computer your agents can work on. Add this Mac, or another computer you own, and agents run there while you keep working.</p>
          <div className="mt-6 flex flex-wrap gap-2">
            <Button size="lg" render={<Link to="/machines" />} data-tree-add-this-mac="">
              Add this Mac
            </Button>
            <Button size="lg" variant="outline" onClick={() => setAdding(true)}>
              Add another computer
            </Button>
          </div>
          <p className="mt-5 text-[13px] text-muted-foreground">
            Or{" "}
            <button type="button" className="underline underline-offset-2 hover:text-foreground" onClick={newSpace} data-tree-new-local="">
              make a new Linux or Mac computer on this Mac
            </button>
            .
          </p>
        </div>
        {sheet}
      </Page>
    );
  }

  const open = (n: TreeNode) => (n.space ? void navigate({ to: "/spaces/$spaceId", params: { spaceId: n.space.id } }) : void navigate({ to: "/machines" }));
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
    void done.then(() => toast(`Renamed to ${name}`, { type: "success" }), toastError(`Couldn't rename ${n.name}`));
  };
  const viewRemove = (n: TreeNode) => machines.view?.machines.find((m) => m.id === n.machine?.id)?.remove ?? null;
  const menu = (n: TreeNode): MenuItem[] => {
    const items: MenuItem[] = [{ label: "Open", onSelect: () => open(n) }];
    if (canRename(n)) items.push({ label: "Rename", onSelect: () => setEditing({ key: n.key, name: n.name }) });
    if (n.settingsId && !n.isDefault && !defaults.unsupported)
      items.push({ label: "Make this my default", onSelect: () => void defaults.makeDefault(n.settingsId!).catch(toastError("Couldn't change your default Space")) });
    if (n.kind === "computer" && !n.current && n.machine) {
      const r = viewRemove(n);
      items.push({ label: `Remove ${n.name}`, destructive: true, disabled: r ? !r.enabled : false, onSelect: () => setRemoving(n) });
    }
    if (n.kind === "space" && n.space?.access?.canDelete !== false) items.push({ label: "Delete", destructive: true, onSelect: () => setDeleting(n) });
    return items;
  };

  const rowFor = (n: TreeNode, depth: 0 | 1) => {
    const folded = collapsed.has(n.key);
    const isEditing = editing?.key === n.key;
    return (
      <div key={n.key} role="row" data-tree-row={n.key} data-tree-kind={n.kind} data-dim={n.dim ? "" : undefined} className={cn(COLS, "min-h-12 px-4 py-1.5 text-[14px]", n.dim && "text-muted-foreground")}>
        <div role="cell" className={cn("flex min-w-0 items-center gap-2.5", depth === 1 && "pl-8", isEditing && "col-span-2")}>
          {depth === 0 && n.children.length > 0 ? (
            <button
              type="button"
              aria-label={folded ? `Show the Spaces on ${n.name}` : `Hide the Spaces on ${n.name}`}
              aria-expanded={!folded}
              onClick={() => toggleCollapsed(n.key)}
              className="-ml-5 flex size-4 items-center justify-center text-muted-foreground hover:text-foreground"
              data-tree-fold={n.key}
            >
              <ChevronRightIcon className={cn("size-3.5 transition-transform", !folded && "rotate-90")} />
            </button>
          ) : null}
          <TreeIcon node={n} className={cn(n.dim && "opacity-50")} />
          {isEditing ? (
            <Input
              autoFocus
              aria-label={`New name for ${n.name}`}
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
            >
              {n.name}
            </button>
          )}
        </div>
        {isEditing ? (
          <div role="cell" className="col-span-3 text-right text-[13px] text-muted-foreground">
            Enter to save, Esc to cancel
          </div>
        ) : (
          <>
            <div role="cell" className="flex min-w-0 items-center gap-2 text-[13px] text-muted-foreground" data-tree-status={n.key}>
              <TreeDot tone={n.tone} />
              <span className="truncate">{n.status}</span>
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
                  <button type="button" aria-label={`More for ${n.name}`} className="flex size-7 items-center justify-center rounded-lg text-muted-foreground hover:bg-foreground/5 hover:text-foreground" data-tree-menu={n.key}>
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

  const single = tree.computers === 1 && tree.spaces === 0;
  return (
    <Page>
      <PageHeader title="Spaces" description={editing ? "Double-click a name, or pick Rename from its menu." : treeSummary(tree)} actions={addButton} />
      <div role="table" aria-label="Your Spaces" data-spaces-tree="">
        <div role="row" className={cn(COLS, "px-4 pb-2 text-[13px] text-muted-foreground")}>
          <div role="columnheader">Name</div>
          <div role="columnheader">Status</div>
          <div role="columnheader">Who's on it</div>
          <div role="columnheader">Agent tasks</div>
          <div role="columnheader" aria-label="Actions" />
        </div>
        <GroupedSection role="rowgroup">{tree.roots.flatMap((r) => [rowFor(r, 0), ...(collapsed.has(r.key) ? [] : r.children.map((c) => rowFor(c, 1)))])}</GroupedSection>
      </div>
      {editing ? <p className="mt-4 text-[13px] text-muted-foreground">{RENAME_NOTE}</p> : null}
      {single ? (
        // f7: only This Mac. Agents are better off on a computer you're not using.
        <GroupedSection className="mt-8" data-tree-single="">
          <div className="flex items-center gap-4 px-5 py-4">
            <HardDriveIcon className="size-6 shrink-0 text-muted-foreground" strokeWidth={1.5} />
            <div className="min-w-0 flex-1">
              <div className="text-[15px] font-medium">Add a computer you're not using</div>
              <div className="text-[13px] text-muted-foreground">A Mac mini or PC you own. Agents work there and This Mac stays yours.</div>
            </div>
            <Button variant="outline" onClick={() => setAdding(true)}>
              Add a computer
            </Button>
          </div>
        </GroupedSection>
      ) : null}
      {sheet}
      {removing ? <RemoveComputerDialog node={removing} onClose={() => setRemoving(null)} /> : null}
      <ConfirmDialog
        open={deleting !== null}
        onOpenChange={(o) => !o && setDeleting(null)}
        title={deleting ? `Delete ${deleting.name}?` : ""}
        description="It's deleted with its files. This can't be undone."
        confirmLabel="Delete"
        destructive
        cancelFirst
        onConfirm={() => {
          const n = deleting!;
          setDeleting(null);
          void deleteSpace(n.space!.id).then(() => toast(`Deleted ${n.name}`, { type: "success" }), toastError(`Couldn't delete ${n.name}`));
        }}
      />
    </Page>
  );
}

/**
 * f9: removing a computer names each Space made on it and any task that
 * stops. They are only detached unless "Also delete" is ticked: then each is
 * deleted through the computer first, and the relay forgets them with it.
 */
export function RemoveComputerDialog({ node, onClose }: { node: TreeNode; onClose: () => void }) {
  const { deleteSpace } = useSpaces();
  const machines = useMachines();
  const [alsoDelete, setAlsoDelete] = useState(false);
  const [busy, setBusy] = useState(false);
  const text = removeText(node);
  const r = machines.view?.machines.find((m) => m.id === node.machine?.id)?.remove ?? null;
  const run = async () => {
    setBusy(true);
    try {
      if (alsoDelete) {
        for (const c of node.children) {
          if (!c.space) continue;
          try {
            await deleteSpace(c.space.id);
          } catch (e) {
            // A Space that couldn't be deleted stops here: the computer stays, so nothing is half-removed.
            toastError(`Couldn't delete ${c.name}, so ${node.name} wasn't removed`)(e);
            return;
          }
        }
      }
      await machines.remove(r?.machine ?? node.machine!.id, r?.device ?? null, alsoDelete);
      toast(`Removed ${node.name}`, { type: "success" });
      onClose();
    } catch (e) {
      toastError(`Couldn't remove ${node.name}`)(e);
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
              {node.children.map((c) => (
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
            Cancel
          </Button>
          <Button variant="destructive" disabled={busy} onClick={() => void run()} data-tree-remove-confirm="">
            {text.confirm}
          </Button>
        </div>
      </DialogPopup>
    </Dialog>
  );
}
