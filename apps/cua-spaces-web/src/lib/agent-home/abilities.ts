// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * What an agent can ask for in its home (CUA-2630), as one small interface.
 *
 * The agent home is where an ask-first request starts: the root of that
 * request's tree. The container version (`containerHome`) is the
 * slim Linux image (`linux:24.04-slim`); a wasm version can replace it later by implementing
 * `AgentHomeRuntime` and listing the same abilities, with no change to the
 * agent, the approvals or the grants: all three only know ability ids.
 *
 * Three abilities, each asked for by the agent and approved by the person:
 *
 * - `spaces.list`: see Spaces. Only the ones the person @mentioned until
 *   they approve listing all (once, remembered per agent);
 * - `spaces.spawn`: make a child Space with an OS, an image and a budget,
 *   inside the tree's budget. Always available in a home; the person
 *   already set the budget;
 * - `machines.use`: use one of the person's machines (their Mac, say).
 *   "Let this agent use this Mac?" once, remembered per agent. An
 *   @mentioned Space or machine counts as the grant.
 */

import type { SpaceOs } from "@/bridge";

export type AbilityId = "spaces.list" | "spaces.spawn" | "machines.use";

export const ABILITY_IDS: readonly AbilityId[] = ["spaces.list", "spaces.spawn", "machines.use"];

/** One ability, as the agent is told about it and as the person is asked. */
export interface AbilityDescriptor {
  id: AbilityId;
  /** How the agent calls it in this home (a command, a tool). */
  call: string;
  /** What the agent is told it does. */
  summary: string;
  /** The person is asked before it works beyond what they already named. */
  asksFirst: boolean;
}

/** What `spaces.spawn` takes: the child's OS, image and budget. */
export interface SpawnRequest {
  os: SpaceOs;
  image?: string;
  name?: string;
  /** Dollars per hour this child may cost; it must fit what is left of the tree's budget. */
  budgetUsdPerHour?: number;
}

/** The grants one agent holds, remembered per agent. */
export interface AgentGrants {
  /** The person let this agent list every Space. */
  listAll: boolean;
  /** Machines (ids) the person let this agent use. */
  machines: string[];
}

export const NO_GRANTS: AgentGrants = { listAll: false, machines: [] };

/** What the person already named in the request: @mentions and a place. */
export interface HomeMentions {
  /** Spaces @mentioned, by id and name. */
  spaces: { id: string; name: string }[];
  /** Machines @mentioned or asked for ("on my Mac", "connect to <x>"), by id and name. */
  machines: { id: string; name: string }[];
}

export const NO_MENTIONS: HomeMentions = { spaces: [], machines: [] };

/** What a new home is made with. */
export interface HomeSetup {
  agent: string;
  grants: AgentGrants;
  mentions: HomeMentions;
}

/** The create options a runtime asks for (the SDK's create config, plus the home's labels). */
export interface HomeCreate {
  image: string;
  on: "local";
  kind: "container";
  name: string;
  cpus: number;
  memoryMb: number;
  spacesd: true;
  /** The home may make child Spaces (the spawn capability). */
  children: true;
  labels: Record<string, string>;
}

/** An agent home: a container today, wasm later. */
export interface AgentHomeRuntime {
  readonly kind: "container" | "wasm";
  /** The abilities this home offers, in the words the agent is told. */
  abilities(): AbilityDescriptor[];
  /** What to create for `setup`. */
  create(setup: HomeSetup, name: string): HomeCreate;
}
