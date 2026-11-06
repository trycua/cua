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

import { resolveDefaultSpace, type DefaultSpace, type DefaultSpaceStep, type SpacesSettings } from "@/bridge";

import type { NewKind, TaskWhere } from "./ask-first";
import { defaultCandidates, findNode, type SpacesTree, type TreeNode } from "./spaces-tree";
import { thisComputerLabel } from "./host-labels";

/** How a computer in "Where a task goes" stands: it runs the task, it could, it's online but has no desktop to run in, or it's offline. */
export type WhereStepState = "used" | "ready" | "no-desktop" | "offline";

/** One line of "Where a task goes" (f2). */
export interface WhereStep {
  name: string;
  /** "Your default. Offline now", "Next online computer. Used now", "Last resort". */
  note: string;
  online: boolean;
  used: boolean;
  state: WhereStepState;
  node: TreeNode | null;
}

export interface TreeDefault {
  where: TaskWhere;
  /** The row it runs on; null: This Mac before it's in the tree. */
  node: TreeNode | null;
  /** The default can't take the task (offline, or it shares no desktop): the line under Home's box and the order tried. Null otherwise. */
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

/** Why the default can't take a task. */
export type DefaultProblem = "offline" | "no-desktop";

/** The parts of the line under Home's box; `fallbackSentence` puts them together. */
export interface FallbackParts {
  /** The default's own name, never "your default". */
  defaultName: string;
  problem: DefaultProblem;
  /** The computer it runs on instead; null: This Mac. */
  usedName: string | null;
  /** Another computer is online but shares no desktop to run in, so "no other computer is online" would be wrong. */
  othersOnline: boolean;
}

const PROBLEM: Record<DefaultProblem, string> = { offline: "is offline", "no-desktop": "is online but doesn't share its desktop" };

/** The line under Home's box, built from its parts so a name or a phrase is never said twice. */
export function fallbackSentence({ defaultName, problem, usedName, othersOnline }: FallbackParts): string {
  const why = `${defaultName}, your default, ${PROBLEM[problem]}`;
  if (usedName) return `${why}. This runs on ${usedName}, the next computer that can take it. If none can, it runs on ${thisComputerLabel()}.`;
  const others = othersOnline ? "no other computer can take it now" : "no other computer is online";
  return `${why}, and ${others}. This runs on ${thisComputerLabel()}.`;
}

const stepNote = (role: string, state: WhereStepState): string => {
  if (role === "here") return state === "used" ? "Last resort. Used now" : "Last resort";
  if (role === "default") return state === "offline" ? "Your default. Offline now" : state === "no-desktop" ? "Your default. Doesn't share its desktop" : "Your default";
  return state === "used" ? "Next online computer. Used now" : state === "ready" ? "Next to try" : state === "no-desktop" ? "Online, doesn't share its desktop" : "Offline now";
};

/**
 * Where a task goes unless you pick: the resolution's Space, as a place to
 * run. A computer that's online but shares no desktop can't take the task, so
 * it's passed over for the next one in the order that can, then This Mac.
 * Never a new computer. A saved default that's no longer in the tree (the
 * computer was removed) counts as no default at all.
 */
export function treeDefault(tree: SpacesTree, settings: SpacesSettings | null, resolved: DefaultSpace | null): TreeDefault {
  const r = resolvedDefault(tree, settings, resolved);
  const here = tree.roots.find((n) => n.current) ?? null;
  const defNode = r.defaultSpace ? (findNode(tree, r.defaultSpace) ?? null) : null;

  // The order with each computer's own row; computers no longer in the tree (removed since) are left out.
  const order = r.order
    .map((o) => ({ o, n: o.role === "here" ? here : (findNode(tree, o.space) ?? null) }))
    .filter(({ o, n }) => o.role === "here" || n !== null);
  const stateOf = (o: DefaultSpaceStep, n: TreeNode | null): Exclude<WhereStepState, "used"> => {
    if (o.role === "here") return "ready";
    if (!n || !o.online || !n.online) return "offline";
    return nodeWhere(n) ? "ready" : "no-desktop";
  };
  const states = order.map(({ o, n }) => stateOf(o, n));

  // Start from the resolution's pick (null: This Mac) and take the first that can run the task.
  const pickedAt = r.space === null ? -1 : order.findIndex(({ o, n }) => o.role !== "here" && (o.space === r.space || n?.key === r.space || n?.settingsId === r.space));
  const usedAt = pickedAt < 0 ? -1 : order.findIndex(({ o }, i) => i >= pickedAt && o.role !== "here" && states[i] === "ready");
  const usedNode = usedAt >= 0 ? order[usedAt]!.n : null;
  const usedWhere = usedNode ? nodeWhere(usedNode) : null;
  const where: TaskWhere = usedWhere ?? { kind: "this-mac" };
  const node = where.kind === "this-mac" ? here : usedNode;

  const defIndex = order.findIndex(({ o }) => o.role === "default");
  const defState = defIndex >= 0 ? states[defIndex]! : null;
  // No line while the default takes the task, or there's no default that still exists (or it's This Mac).
  if (!defNode || defNode.current || defState === null || defState === "ready") return { where, node, fallback: null };

  const usedHere = where.kind === "this-mac";
  const steps: WhereStep[] = order.map(({ o, n }, i) => {
    const used = o.role === "here" ? usedHere : i === usedAt && !usedHere;
    const state: WhereStepState = used ? "used" : states[i]!;
    const name = o.role === "here" ? thisComputerLabel() : (n?.name ?? o.name);
    return { name, note: stepNote(o.role, state), online: state !== "offline", used, state, node: n };
  });
  const line = fallbackSentence({
    defaultName: defNode.name,
    problem: defState,
    usedName: usedHere ? null : (usedNode?.name ?? null),
    othersOnline: steps.some((s, i) => i !== defIndex && s.state === "no-desktop"),
  });
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
