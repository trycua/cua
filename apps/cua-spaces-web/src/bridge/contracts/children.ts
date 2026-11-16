// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Child Spaces as the Spaces page and the Space page read them: which
 * Spaces belong to which, whether a Space may make children, its tree's
 * budget and the commands it keeps running. From the host
 * (`bridge/children.ts`, over `spaces.tree`, `children.*`, `budget.*` and
 * `services.*`) or the demo's memory (`?demo=network`). All of it is the
 * Child Spaces experiment's (Settings, Experiments).
 */

/** One Space's place in its tree. */
export interface TreeNode {
  id: string;
  /** Its parent's id; absent for a top-level Space. */
  parent?: string;
  /** 0 for a top-level Space, 1 for its children, ... */
  depth: number;
  /** It may make child Spaces of its own. */
  childrenOn: boolean;
  /** It is an agent's home: the agent (harness id) it was made for. */
  home?: string;
}

/** What a tree's children may hold and cost. Unset: the default. */
export interface TreeBudgetLimits {
  maxSpaces?: number;
  maxVcpus?: number;
  maxMemoryMb?: number;
  usdPerHour?: number;
  usdTotal?: number;
}

/** A tree's budget and what its children use now. */
export interface TreeBudget {
  root: string;
  /** What the owner set (unset fields take the default). */
  set: TreeBudgetLimits;
  maxSpaces: number;
  maxVcpus: number;
  maxMemoryMb: number;
  usdPerHour: number;
  usdTotal?: number;
  spaces: number;
  vcpus: number;
  memoryMb: number;
  usdPerHourUsed: number;
  usdSpent: number;
}

/** A command a Space keeps running. */
export interface KeptService {
  name: string;
  command: string;
  /** `starting`, `running`, `unhealthy`, `backoff`, `stopped`, `exited`, `failed`. */
  state: string;
  restarts: number;
  /** How the last process ended, when one did. */
  lastEnd: string;
  /** The last failed health check. */
  probeError: string;
}

export type ServiceAction = "start" | "stop" | "restart" | "remove";

/** The host's store (`DataAdapter.children`). */
export interface ChildrenPreview {
  /** A Space's place in its tree; null before the first read. */
  node(spaceId: string): TreeNode | null;
  /** A Space's direct children, by id. */
  childrenOf(spaceId: string): string[];
  /** The agent homes the host knows (from their labels), by id: the ones made for `agent` first. */
  homesOf(agent: string): string[];
  /** Lets a Space make children, or stops it. */
  setChildrenOn(spaceId: string, on: boolean): void;
  /** The budget of `spaceId`'s tree; null before the first read. */
  budget(spaceId: string): TreeBudget | null;
  setBudget(spaceId: string, limits: TreeBudgetLimits): void;
  /** The commands a Space keeps running. */
  services(spaceId: string): KeptService[];
  controlService(spaceId: string, name: string, action: ServiceAction): void;
  /** The last change the host refused, in its words. */
  error(spaceId: string): string | null;
  subscribe(listener: () => void): () => void;
  /** Bumps on every change (for `useSyncExternalStore`). */
  version(): number;
}
