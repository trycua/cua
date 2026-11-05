// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { Menu as MenuPrimitive } from "@base-ui/react/menu";
import { CheckIcon, PlusIcon } from "lucide-react";
import { useMemo, type ReactElement, type ReactNode } from "react";

import type { SpaceOs } from "@/bridge";
import { TreeDot, TreeIcon } from "@/components/spaces/tree-parts";
import { useRunningBySpace, useSpacesTreeData } from "@/components/spaces/use-spaces-tree";
import { toastError } from "@/components/ui/toast";
import type { NewKind, TaskWhere } from "@/lib/ask-first";
import { newSpaceTargets, nodeWhere, treeDefault, type TreeDefault } from "@/lib/default-where";
import { findNode, treeRows, type SpacesTree, type TreeNode } from "@/lib/spaces-tree";
import { cn } from "@/lib/utils";

const OS_WORD: Record<SpaceOs, string> = { linux: "Linux", macos: "Mac", windows: "Windows", unknown: "" };

/** The picker's closing line: nothing is made without being picked. */
export const NEVER_MADE = "A new computer is never made unless you pick one.";

export interface TreeWhere {
  tree: SpacesTree;
  /** Where a task goes unless you pick (the default, or the next online computer). */
  def: TreeDefault;
  makeDefault: ((settingsId: string) => void) | null;
}

/** Home's "in" with Spaces as a tree (Labs): the tree and its default; null while the switch is off or the lists load. */
export function useTreeWhere(): TreeWhere | null {
  const running = useRunningBySpace();
  const { on, tree, defaults } = useSpacesTreeData(running);
  const { settings, resolved, unsupported } = defaults;
  const def = useMemo(() => (tree ? treeDefault(tree, settings, resolved) : null), [tree, settings, resolved]);
  if (!on || !tree || !def) return null;
  return {
    tree,
    def,
    makeDefault: unsupported ? null : (id) => void defaults.makeDefault(id).catch(toastError("Couldn't change your default Space")),
  };
}

/** The row a choice is, when it is one of the tree's. */
export function whereNode(tree: SpacesTree, where: TaskWhere | null): TreeNode | undefined {
  if (!where) return undefined;
  if (where.kind === "this-mac") return tree.roots.find((r) => r.current);
  if (where.kind === "space") return findNode(tree, where.space.id);
  return undefined;
}

function agentsLine(n: Pick<TreeNode, "running">): string | null {
  return n.running === 0 ? null : n.running === 1 ? "1 agent working" : `${n.running} agents working`;
}

/** A row's second line in the picker. */
export function pickerNote(n: TreeNode): string {
  if (!n.online) return n.status.replace(/, Default$/, "");
  const agents = agentsLine(n);
  if (n.current) return agents ?? "Works on your own files and apps";
  if (n.kind === "computer" && n.people.length > 1) return agents ? `Shared with your team. ${agents}` : "Shared with your team";
  return agents ?? "On now";
}

function Row({
  icon,
  title,
  note,
  dot,
  selected,
  disabled,
  trailing,
  indent,
  onSelect,
  attrs,
}: {
  icon: ReactNode;
  title: string;
  note?: string | null;
  dot?: ReactNode;
  selected?: boolean;
  disabled?: boolean;
  trailing?: ReactNode;
  indent?: boolean;
  onSelect: () => void;
  attrs?: Record<`data-${string}`, string>;
}) {
  return (
    <MenuPrimitive.Item
      onClick={onSelect}
      disabled={disabled}
      {...attrs}
      className={cn(
        "group/row flex min-h-11 cursor-default items-center gap-3 rounded-lg px-3 py-1.5 outline-none data-disabled:opacity-50 data-highlighted:bg-foreground/[0.06]",
        selected && "bg-foreground/[0.06]",
        indent && "pl-9",
      )}
    >
      {icon}
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 text-[14px] font-medium">
          <span className="truncate">{title}</span>
          {dot}
        </div>
        {note ? <div className="truncate text-[12px] text-muted-foreground">{note}</div> : null}
      </div>
      {trailing}
      {selected ? <CheckIcon className="size-4 shrink-0" /> : null}
    </MenuPrimitive.Item>
  );
}

/**
 * "Where it runs" as a tree (f1): what's picked first, then your Spaces under
 * the computers they're on, each with "Default" or "Make this my default",
 * then "New Space on ..." (the only rows that make a computer).
 */
export function TreeWherePicker({
  tw,
  where,
  onWhere,
  kinds,
  trigger,
  open,
  onOpenChange,
}: {
  tw: TreeWhere;
  where: TaskWhere | null;
  onWhere: (w: TaskWhere) => void;
  kinds: NewKind[];
  trigger: ReactElement;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}) {
  const { tree, def, makeDefault } = tw;
  const chosen = whereNode(tree, where);
  const rows = treeRows(tree);
  const okKinds = kinds.filter((k) => !k.blocked);
  const head = chosen ?? (where?.kind === "new" ? null : def.node);
  return (
    <MenuPrimitive.Root {...(open === undefined ? {} : { open })} onOpenChange={(o) => onOpenChange?.(o)}>
      <MenuPrimitive.Trigger render={trigger} />
      <MenuPrimitive.Portal>
        <MenuPrimitive.Positioner sideOffset={6} align="start" className="z-50 outline-none">
          {/* design-ok: a pop-up menu's edge, as the other pickers' */}
          <MenuPrimitive.Popup data-tree-where="" className="glass max-h-[min(640px,80vh)] w-[480px] origin-(--transform-origin) overflow-y-auto rounded-lg border p-2 text-popover-foreground shadow-float outline-none transition-[opacity,scale] data-ending-style:scale-95 data-ending-style:opacity-0 data-starting-style:scale-95 data-starting-style:opacity-0"
          >
            <p className="px-3 pt-2 pb-1 text-[12px] font-medium text-muted-foreground">Where it runs</p>
            {head ? (
              <Row
                icon={<TreeIcon node={head} />}
                title={head.name}
                dot={<TreeDot tone={head.tone} />}
                note={head.isDefault ? `Default. ${head.online ? "Online" : "Offline"}${agentsLine(head) ? `, ${agentsLine(head)}` : ""}` : pickerNote(head)}
                selected
                onSelect={() => {
                  const w = nodeWhere(head);
                  if (w) onWhere(w);
                }}
                attrs={{ "data-tree-where-current": head.key }}
              />
            ) : null}
            <p className="px-3 pt-4 pb-1 text-[12px] font-medium text-muted-foreground">Your Spaces</p>
            {rows.map((n) => {
              const w = nodeWhere(n);
              return (
                <Row
                  key={n.key}
                  icon={<TreeIcon node={n} className={cn(n.dim && "opacity-50")} />}
                  title={n.name}
                  dot={<TreeDot tone={n.tone} />}
                  note={n.kind === "space" && n.online ? agentsLine(n) : pickerNote(n)}
                  indent={n.parentKey !== null}
                  disabled={!w || !n.online}
                  onSelect={() => w && onWhere(w)}
                  attrs={{ "data-tree-where-row": n.key }}
                  trailing={
                    n.isDefault ? (
                      <span data-tree-default="" className="shrink-0 text-[12px] text-muted-foreground">
                        Default
                      </span>
                    ) : n.settingsId && makeDefault ? (
                      <button
                        type="button"
                        data-tree-make-default={n.settingsId}
                        className="shrink-0 text-[12px] text-brand-strong opacity-0 group-data-highlighted/row:opacity-100 hover:underline focus-visible:opacity-100"
                        onClick={(e) => {
                          e.stopPropagation();
                          e.preventDefault();
                          makeDefault(n.settingsId!);
                        }}
                      >
                        Make this my default
                      </button>
                    ) : null
                  }
                />
              );
            })}
            <div className="h-3" aria-hidden />
            {newSpaceTargets(tree).flatMap((t) =>
              t.on
                ? [
                    <Row
                      key={t.key}
                      icon={<PlusIcon className="size-4 text-muted-foreground" />}
                      title={`New Space on ${t.name}`}
                      note="A Linux computer, made fresh"
                      selected={where?.kind === "new" && where.on?.machine === t.on.machine}
                      onSelect={() => onWhere({ kind: "new", os: "linux", on: t.on! })}
                      attrs={{ "data-tree-new-on": t.on.machine }}
                    />,
                  ]
                : okKinds.map((k) => (
                    <Row
                      key={`${t.key}:${k.os}`}
                      icon={<PlusIcon className="size-4 text-muted-foreground" />}
                      title={`New Space on ${t.name}`}
                      note={`A ${OS_WORD[k.os]} computer, made fresh. ${k.note}`}
                      selected={where?.kind === "new" && !where.on && where.os === k.os}
                      onSelect={() => onWhere({ kind: "new", os: k.os })}
                      attrs={{ "data-tree-new-here": k.os }}
                    />
                  )),
            )}
            {/* design-ok: the menu's footer rule, as the other pickers' */}
            <div className="mt-1 border-t px-3 pt-2 pb-1 text-[12px] text-muted-foreground">{NEVER_MADE}</div>
          </MenuPrimitive.Popup>
        </MenuPrimitive.Positioner>
      </MenuPrimitive.Portal>
    </MenuPrimitive.Root>
  );
}

/** f2: the default is offline. One line says why, then the order tried. */
export function FallbackLine({ fallback }: { fallback: NonNullable<TreeDefault["fallback"]> }) {
  return (
    <div data-tree-fallback="" className="flex flex-col gap-3 text-[14px]">
      <p className="flex items-start gap-2 text-muted-foreground">
        <span className="mt-2 size-1.5 shrink-0 rounded-full bg-muted-foreground/50" aria-hidden />
        {fallback.line}
      </p>
      <div>
        <p className="mb-2 text-[13px] text-muted-foreground">Where a task goes</p>
        <ol className="flex flex-col gap-2">
          {fallback.steps.map((s, i) => (
            <li key={`${s.name}:${i}`} className="flex items-center gap-3" data-tree-fallback-step={s.used ? "used" : s.online ? "online" : "offline"}>
              <span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-foreground/[0.06] text-[12px] text-muted-foreground">{i + 1}</span>
              {s.node ? <TreeIcon node={s.node} /> : null}
              <span className={cn(!s.online && "text-muted-foreground")}>{s.name}</span>
              <span className="text-[13px] text-muted-foreground">{s.note}</span>
            </li>
          ))}
        </ol>
      </div>
      <p className="text-[13px] text-muted-foreground">{NEVER_MADE}</p>
    </div>
  );
}
