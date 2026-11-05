// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Where a new task from Home goes with Spaces as a tree on (CUA-3413): your
 * default Space, or when it's offline your next online computer, then This
 * Mac. It replaces `defaultWhere()` (a new Linux computer) while the Labs
 * switch is on. A new computer is never made unless you pick
 * "New Space on ..." yourself.
 *
 * The relay resolves the order (`spaces.resolveDefault`); a host that can't
 * ask it resolves the same way over the tree (`resolveDefaultSpace`).
 */

import { resolveDefaultSpace, type DefaultSpace, type SpacesSettings } from "@/bridge";

import type { NewKind, TaskWhere } from "./ask-first";
import { defaultCandidates, findNode, type SpacesTree, type TreeNode } from "./spaces-tree";

/** One line of "Where a task goes" (f2). */
export interface WhereStep {
  name: string;
  /** "Your default. Offline now", "Next online computer. Used now", "Last resort". */
  note: string;
  online: boolean;
  used: boolean;
  node: TreeNode | null;
}

export interface TreeDefault {
  where: TaskWhere;
  /** The row it runs on; null: This Mac before it's in the tree. */
  node: TreeNode | null;
  /** The default is offline: the line under Home's box and the order tried. Null otherwise. */
  fallback: { line: string; steps: WhereStep[] } | null;
}

/** A row as a place to run: This Mac itself, or the Space row (a computer's own desktop, or a Space on it). Null: nothing to run in. */
export function nodeWhere(node: TreeNode): TaskWhere | null {
  if (node.current) return { kind: "this-mac" };
  return node.space ? { kind: "space", space: node.space } : null;
}

/** The relay's answer when it gave one, else the same rule over the tree. */
export function resolvedDefault(tree: SpacesTree, settings: SpacesSettings | null, resolved: DefaultSpace | null): DefaultSpace {
  if (resolved) return resolved;
  // The tree's own default stands in for an unset one ("Default" is shown on it).
  const def = settings?.defaultSpace ?? (tree.defaultKey && findNode(tree, tree.defaultKey)?.settingsId) ?? null;
  return resolveDefaultSpace({ defaultSpace: def, fallback: settings?.fallback ?? [], fallbackOn: settings?.fallbackOn ?? true }, defaultCandidates(tree));
}

/**
 * Where a task goes unless you pick: the resolution's Space, as a place
 * to run. Anything that can't be run in (gone from the list, no desktop
 * to start in) goes to This Mac. Never a new computer.
 */
export function treeDefault(tree: SpacesTree, settings: SpacesSettings | null, resolved: DefaultSpace | null): TreeDefault {
  const r = resolvedDefault(tree, settings, resolved);
  const picked = findNode(tree, r.space);
  const where = picked ? (nodeWhere(picked) ?? ({ kind: "this-mac" } as const)) : ({ kind: "this-mac" } as const);
  const node = where.kind === "this-mac" ? (tree.roots.find((n) => n.current) ?? null) : (picked ?? null);
  const usedKey = node?.key ?? null;
  const defOffline = r.defaultSpace !== null && r.defaultOnline === false;
  if (!defOffline) return { where, node, fallback: null };

  const steps: WhereStep[] = r.order.map((o) => {
    const n = findNode(tree, o.space) ?? null;
    const used = o.role === "here" ? where.kind === "this-mac" : where.kind !== "this-mac" && n !== null && n.key === usedKey;
    const name = o.role === "here" ? "This Mac" : (n?.name ?? o.name);
    const note =
      o.role === "default" ? (o.online ? "Your default" : "Your default. Offline now") : o.role === "here" ? (used ? "Last resort. Used now" : "Last resort") : used ? "Next online computer. Used now" : o.online ? "Next to try" : "Offline now";
    return { name, note, online: o.online, used, node: n };
  });
  const defName = steps.find((s) => s.note.startsWith("Your default"))?.name ?? findNode(tree, r.defaultSpace)?.name ?? "Your default";
  const usedStep = steps.find((s) => s.used);
  const line =
    where.kind === "this-mac" || !usedStep
      ? `${defName}, your default, is offline, and no other computer is online. This runs on This Mac.`
      : `${defName}, your default, is offline. This runs on ${usedStep.name}, your next online computer. If none is online, it runs on This Mac.`;
  return { where, node, fallback: { line, steps } };
}

/** The pick if it's still there and usable, else the default. A pick that went away never becomes a new computer. */
export function currentTreeWhere(picked: TaskWhere | null, fallback: TaskWhere, kinds: readonly NewKind[], tree: SpacesTree): TaskWhere {
  if (picked?.kind === "this-mac") return picked;
  if (picked?.kind === "new") {
    if (picked.on) return tree.roots.some((r) => r.machine?.id === picked.on!.machine && r.online) ? picked : fallback;
    return kinds.some((k) => k.os === picked.os && !k.blocked) ? picked : fallback;
  }
  if (picked?.kind === "space") {
    const n = findNode(tree, picked.space.id);
    if (n?.online && n.space) return { kind: "space", space: n.space };
  }
  return fallback;
}

/** "New Space on <computer>": This Mac first, then each online computer of yours that makes Spaces. */
export function newSpaceTargets(tree: SpacesTree): { key: string; name: string; on: { machine: string; name: string } | null }[] {
  return tree.roots
    .filter((r) => r.kind === "computer" && (r.current || (r.online && r.machine && r.machine.connection !== "direct")))
    .map((r) => ({ key: r.key, name: r.name, on: r.current || !r.machine ? null : { machine: r.machine.id, name: r.name } }));
}
