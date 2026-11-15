// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Your computers as a tree (CUA-3413, CUA-3551): every computer you added
 * is a top row, and the Spaces made on it sit under it. One pure function
 * builds it for the sidebar, the Computers page, Home's "in" picker and
 * Settings, from what the app already has: the Spaces list (`Space.host`,
 * `provider`), the machines (`Machine.current`, `relayId`) and the agent
 * runs. Nothing new is stored; the default Space is the relay's
 * (`spaces.settings`).
 */

import type { Machine, Space, SpaceOs } from "@/bridge";
import { t } from "@/i18n";
import { ownerLabel, peopleNames } from "@/lib/space-access";
import { machineOs, reachable } from "@/lib/machines";
import { isMachineDesktop, spaceState, type SpaceState } from "@/lib/spaces";
import { relativeTime } from "@/lib/utils";
import { thisComputerLabel } from "./host-labels";

/** How a row reads in the Status column (and its dot). */
export type TreeTone = "online" | "offline" | "stopped" | "busy";

export interface TreeNode {
  /** A stable key: the computer's Space id, or the Space's. */
  key: string;
  kind: "computer" | "space";
  name: string;
  os: SpaceOs | null;
  /** The id the relay knows it by (`relay:<machine>`, `relay:<machine>/<name>`): it can be the default. Null: it can't (a Space on This Mac only, or a computer added by address). */
  settingsId: string | null;
  /** The Spaces list's row: what opens it and what a task runs in. Null: nothing to open (a computer that shares no desktop). */
  space: Space | null;
  /** The machine, for a computer. */
  machine: Machine | null;
  /** This Mac. */
  current: boolean;
  /** Usable now: online, and (for a Space) its computer is too. */
  online: boolean;
  tone: TreeTone;
  /** "Online", "Online, Default", "Offline, seen 20 minutes ago", "Offline" (a Space on an offline computer), "Stopped". */
  status: string;
  isDefault: boolean;
  /** Its computer is offline: the row is greyed out until it's back. */
  dim: boolean;
  /** Running agent tasks: its own, and for a computer its Spaces' too. */
  running: number;
  /** "1 running", "Paused" while its computer is offline, or "" (an empty cell) when there are none. */
  tasks: string;
  /** Who's on it: the owner (or you) first, then who it's shared with. */
  people: string[];
  /** What it sits under: the computer it was made on, or for a child Space the Space that made it; null for a computer, or a Space whose computer isn't listed. */
  parentKey: string | null;
  /** For a computer, the Spaces made on it that no other Space made; for a Space, the child Spaces it made. */
  children: TreeNode[];
  /** 0 for a computer (or a Space with no listed computer), 1 for a Space on a computer, 2 for its children, ... */
  depth: number;
  /** A computer only: the Space page its row opens. Its own desktop's, while it shares it (or, for This Mac, while it can share it and does not: that page makes it one button to start or stop). Null: the computer's own page (its settings), for a computer that cannot share (a cloud computer, one set up to provide Spaces only, one not set up, one that is offline or not sharing). */
  opens: string | null;
}

export interface SpacesTree {
  /** Computers (This Mac first), then any Space whose computer isn't listed. */
  roots: TreeNode[];
  computers: number;
  /** Spaces made on the computers (and any whose computer isn't listed). */
  spaces: number;
  online: number;
  /** The default, marked "Default" (the setting, else the first computer that isn't this Mac, else This Mac). */
  defaultKey: string | null;
  /** Spaces that are running now (the page's line counts them). */
  spacesRunning: number;
}

export interface TreeInput {
  spaces: readonly Space[];
  machines: readonly Machine[];
  /** The stored default (`spaces.settings`); null: unset. */
  defaultSpace: string | null;
  /** Running agent tasks by Space id. */
  running?: ReadonlyMap<string, number>;
  /** Your own name or email, for "Who's on it". */
  me?: string | null;
  now?: number;
  /** What the relay said about each computer when it last resolved the default (`spaces.resolveDefault`'s order, by
   * Space id). A computer it calls offline is offline here too, so the tree never shows green where Home says offline
   * (the machines list can lag, as it also counts a connection the relay has not dropped yet). */
  relayOnline?: ReadonlyMap<string, boolean>;
  /** Which Space made which (Child Spaces): child Space id to its parent's. A child whose parent isn't listed sits on its computer like any Space. */
  parents?: ReadonlyMap<string, string>;
  /** Whether another computer says it shares its desktop (`sharesDesktop`); a computer set up to provide Spaces only does not. Unset: it does. */
  shares?: (space: Space) => boolean;
}

/** A computer's own Space id: `relay:<machine>` (This Mac's once it is on the relay), else its list row's. */
export function computerSpaceId(m: Pick<Machine, "id" | "current" | "relayId" | "connection">): string | null {
  if (m.current) return m.relayId ? `relay:${m.relayId}` : null;
  return m.connection === "direct" ? null : `relay:${m.id}`;
}

/** The relay's own online answer per computer Space id, from the resolved default's order (This Mac is not the relay's to answer). */
export function buildRelayOnline(order: readonly { space: string; online: boolean; role?: string }[] | undefined): Map<string, boolean> {
  return new Map((order ?? []).filter((o) => o.role !== "here").map((o) => [o.space, o.online] as const));
}

/** Whether the relay calls this computer offline, whatever the computer list still counts. */
export function relayCallsOffline(
  m: Pick<Machine, "id" | "current" | "relayId" | "connection">,
  relayOnline: ReadonlyMap<string, boolean> | undefined,
): boolean {
  const id = computerSpaceId(m);
  return id !== null && !m.current && relayOnline?.get(id) === false;
}

/**
 * A computer row left in the Spaces list after its machine went (Remove from
 * account, or a machine the relay forgot): `relay:<machine>` named by its raw
 * id, with no machine in the list. It is not a computer to show, and nothing
 * under it is either.
 */
function ghostMachineIds(spaces: readonly Space[], machines: readonly Machine[]): Set<string> {
  const known = new Set(machines.flatMap((m) => [m.id, ...(m.relayId ? [m.relayId] : [])]));
  const ghosts = new Set<string>();
  for (const s of spaces) {
    const m = /^relay:([^/]+)$/.exec(s.id);
    if (m && !known.has(m[1]!) && (s.name === s.id || s.name === m[1])) ghosts.add(m[1]!);
  }
  return ghosts;
}

/** A Space id the relay knows (`relay:<machine>` or `relay:<machine>/<name>`): one that can be the default. */
const relayId = (id: string): string | null => (id.startsWith("relay:") ? id : null);

const spaceWord = (state: SpaceState): string =>
  ({
    running: t("computers.row.online"),
    stopped: t("spaces.status.stopped"),
    creating: t("computers.tree.space_creating"),
    deleting: t("computers.status.deleting"),
    failed: t("computers.tree.space_failed"),
  })[state];

const withDefault = (word: string): string => t("computers.tree.status_default", { status: word });

function offlineWord(m: Pick<Machine, "lastSeen">, now: number): string {
  return m.lastSeen ? t("computers.tree.offline_seen", { when: relativeTime(m.lastSeen * 1000, now) }) : t("computers.row.offline");
}

function tasksText(running: number, waits: boolean): string {
  if (running === 0) return "";
  return waits ? t("computers.tree.tasks_paused") : t("computers.tree.tasks_running", { count: running });
}

function peopleOf(space: Space | null, me: string | null): string[] {
  const access = space?.access;
  const first = access ? (ownerLabel(access) ?? me) : me;
  const rest = access && access.kind === "mine" ? peopleNames(access) : [];
  return [...new Set([first, ...rest].filter((n): n is string => Boolean(n)))];
}

/** Whether a machine row is one of your computers (not only a signed-in device). This Mac counts once it is shared or has Spaces. */
const isComputer = (m: Machine) => !m.device;

/**
 * Builds the tree. Computers come in the machines' order with This Mac
 * first; a Space sits under the computer it was made on (`host`, or
 * `provider: local` for This Mac). A Space whose computer isn't listed (a
 * teammate's, one in your cloud) is a top row of its own after them. While
 * a computer is offline its Spaces wait for it, and their running tasks
 * read "Paused".
 */
export function buildSpacesTree({
  spaces,
  machines,
  defaultSpace,
  running = new Map(),
  me = null,
  now = Date.now(),
  relayOnline,
  parents = new Map(),
  shares = () => true,
}: TreeInput): SpacesTree {
  const ghosts = ghostMachineIds(spaces, machines);
  const onGhost = (s: Space) => [...ghosts].some((g) => s.id === `relay:${g}` || s.id.startsWith(`relay:${g}/`) || s.host === g);
  const listed = spaces.filter((s) => !isMachineDesktop(s) && !onGhost(s));
  const placed = new Set<string>();
  const here = machines.find((m) => m.current && isComputer(m)) ?? null;
  const others = machines.filter((m) => !m.current && isComputer(m));
  const ordered = [...(here ? [here] : []), ...others];

  // The default: the setting when it's listed, else where a task goes while none is set (the relay's rule): the first
  // online computer that isn't this Mac, else This Mac. An offline computer is never shown as a default nobody chose.
  const ids = new Set<string>();
  for (const m of ordered) {
    const id = computerSpaceId(m);
    if (id) ids.add(id);
  }
  for (const s of listed) if (relayId(s.id)) ids.add(s.id);
  const isOnline = (m: Machine) => {
    const id = computerSpaceId(m);
    return m.online && !(id !== null && relayOnline?.get(id) === false);
  };
  const fallbackDefault =
    others
      .filter(isOnline)
      .map(computerSpaceId)
      .find((id): id is string => id !== null) ?? (here ? (computerSpaceId(here) ?? "this-mac") : null);
  const defaultKey = defaultSpace && ids.has(defaultSpace) ? defaultSpace : fallbackDefault;

  const runningOf = (id: string | null | undefined) => (id ? (running.get(id) ?? 0) : 0);

  // Child Spaces nest under the Space that made them, wherever the two are listed (a child is made on its parent's computer).
  // A parent that isn't listed, or a loop, leaves the Space where its computer puts it.
  const listedIds = new Set(listed.map((s) => s.id));
  const madeBy = (s: Space): string | null => {
    const p = parents.get(s.id);
    if (!p || p === s.id || !listedIds.has(p)) return null;
    for (let at: string | undefined = p, hops = 0; at; at = parents.get(at), hops++) {
      if (at === s.id || hops > 32) return null;
    }
    return p;
  };
  const kidsOf = new Map<string, Space[]>();
  for (const s of listed) {
    const p = madeBy(s);
    if (p) kidsOf.set(p, [...(kidsOf.get(p) ?? []), s]);
  }

  const spaceNode = (s: Space, parent: TreeNode | null, computer: TreeNode | null = parent, depth = 1): TreeNode => {
    const waits = computer !== null && !computer.online;
    const state = spaceState(s);
    const settingsId = relayId(s.id);
    const isDefault = settingsId !== null && settingsId === defaultKey;
    const word = waits ? t("computers.row.offline") : spaceWord(state);
    const n = runningOf(s.id);
    const node: TreeNode = {
      key: s.id,
      kind: "space",
      name: s.name,
      os: s.os === "unknown" ? null : s.os,
      settingsId,
      space: s,
      machine: null,
      current: false,
      online: !waits && state === "running",
      tone: waits
        ? "offline"
        : state === "running"
          ? "online"
          : state === "creating" || state === "deleting"
            ? "busy"
            : state === "failed"
              ? "offline"
              : "stopped",
      status: isDefault && !waits ? withDefault(word) : word,
      isDefault,
      dim: waits,
      running: n,
      tasks: tasksText(n, waits),
      people: peopleOf(s, me),
      parentKey: parent?.key ?? null,
      children: [],
      opens: null,
      depth: parent ? depth : 0,
    };
    // Its child Spaces (Child Spaces) sit under it; their tasks count on it too, so a computer's total stays whole.
    // The Spaces a Space made, in the order of their names (kid-a before kid-b), not the order the list came in.
    const kids = [...(kidsOf.get(s.id) ?? [])].sort((a, b) => a.name.localeCompare(b.name, undefined, { numeric: true }));
    node.children = kids.map((c) => spaceNode(c, node, computer, node.depth + 1));
    for (const c of node.children) placed.add(c.key);
    node.running = n + node.children.reduce((sum, c) => sum + c.running, 0);
    node.tasks = tasksText(node.running, waits);
    return node;
  };

  const roots: TreeNode[] = [];
  for (const m of ordered) {
    const own = m.current ? (spaces.find((s) => s.id === "this-mac") ?? null) : (spaces.find((s) => s.id === `relay:${m.id}`) ?? null);
    const id = computerSpaceId(m);
    const key = id ?? (m.current ? "this-mac" : `machine:${m.id}`);
    const kids = listed.filter(
      (s) => !placed.has(s.id) && (m.current ? s.provider === "local" || (m.relayId !== undefined && s.host === m.relayId) : s.host === m.id),
    );
    // This Mac before it is shared, with no Space on it: not a Space yet (the empty state offers "Add this Mac").
    if (m.current && !id && !m.connection && kids.length === 0) continue;
    const online = reachable(m) && !relayCallsOffline(m, relayOnline);
    const isDefault = key === defaultKey;
    const word = online ? t("computers.row.online") : m.online && m.notSharing ? t("computers.row.not_sharing") : offlineWord(m, now);
    const node: TreeNode = {
      key,
      kind: "computer",
      name: m.current ? thisComputerLabel() : m.name,
      os: machineOs(m),
      settingsId: id,
      space: own,
      machine: m,
      current: m.current,
      online,
      tone: online ? "online" : "offline",
      status: isDefault ? withDefault(word) : word,
      isDefault,
      dim: false,
      running: 0,
      tasks: "",
      people: peopleOf(own, me),
      parentKey: null,
      children: [],
      // This Mac opens its desktop's page once it is set up (sharing or not: the page has the button); another computer, while it shares.
      opens: own && (m.current ? Boolean(m.connection) : online && shares(own) && own.sdk?.reachable !== false) ? own.id : null,
      depth: 0,
    };
    // The Spaces no other Space made sit on the computer; the children nest under their parents.
    const top = kids.filter((s) => madeBy(s) === null);
    for (const s of kids) placed.add(s.id);
    node.children = top.map((s) => spaceNode(s, node));
    node.running = runningOf(id) + (m.current ? runningOf("this-mac") : 0) + node.children.reduce((sum, c) => sum + c.running, 0);
    node.tasks = tasksText(node.running, false);
    if (!online && node.running > 0) node.tasks = t("computers.tree.tasks_paused");
    // A computer shared with you: whoever is on its Spaces is on it too.
    if (node.people.length <= 1) node.people = [...new Set([...node.people, ...node.children.flatMap((c) => c.people)])];
    roots.push(node);
  }
  const orphans = listed.filter((s) => !placed.has(s.id) && madeBy(s) === null).map((s) => spaceNode(s, null));

  const all = [...roots, ...roots.flatMap(descendants), ...orphans.flatMap((o) => [o, ...descendants(o)])];
  return {
    roots: [...roots, ...orphans],
    computers: roots.length,
    spaces: all.length - roots.length,
    online: all.filter((n) => n.online).length,
    defaultKey: all.some((n) => n.key === defaultKey) ? defaultKey : null,
    spacesRunning: all.filter((n) => n.kind === "space" && n.online).length,
  };
}

/** Every Space under `node`, children and theirs included, depth first. */
export function descendants(node: Pick<TreeNode, "children">): TreeNode[] {
  return node.children.flatMap((c) => [c, ...descendants(c)]);
}

/** Fewer offline computers than this stay in the list as they are. */
export const OFFLINE_FOLD_MIN = 2;

/**
 * The Spaces page's top rows, with idle offline computers kept apart: an
 * offline computer with no Spaces on it that isn't This Mac or the default
 * folds into one "N offline" row at the end, so an account with many old
 * computers still reads at a glance. Fewer than `OFFLINE_FOLD_MIN` fold
 * into nothing.
 */
export function foldOffline(roots: readonly TreeNode[]): { shown: TreeNode[]; folded: TreeNode[] } {
  const idle = (n: TreeNode) => n.kind === "computer" && !n.online && !n.current && !n.isDefault && n.children.length === 0;
  const folded = roots.filter(idle);
  if (folded.length < OFFLINE_FOLD_MIN) return { shown: [...roots], folded: [] };
  return { shown: roots.filter((n) => !idle(n)), folded };
}

/** Every row, depth first. */
export function treeRows(tree: Pick<SpacesTree, "roots">): TreeNode[] {
  return tree.roots.flatMap((r) => [r, ...descendants(r)]);
}

/** The Computers page's line under its title: a plain count of the list, nothing about one computer. */
export function treeSummary(tree: SpacesTree): string {
  const computers = t("computers.tree.summary_computers", { computers: tree.computers });
  return tree.spacesRunning === 0 ? computers : t("computers.tree.summary_running", { computers: tree.computers, running: tree.spacesRunning });
}

/** A node by key. */
export function findNode(tree: Pick<SpacesTree, "roots">, key: string | null | undefined): TreeNode | undefined {
  return key ? treeRows(tree).find((n) => n.key === key || n.settingsId === key) : undefined;
}
