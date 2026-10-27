// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Who is in a Space, for the thread and its members panel: the agent
 * members (`agent_members`) and the people, derived as DESIGN §3.3 says
 * (you, whoever the Space is shared with, anyone who posted). Also the
 * names the composer's `@` picker offers.
 */

import { DEFAULT_AGENT_RIGHTS, type AgentMember, type AgentRight, type AgentRights, type MemberIdentity, type ShareEntry, type ThreadAuthor, type ThreadEntry, type ThreadRole } from "@/bridge";
import type { MentionCandidate } from "@/lib/mentions";

export interface RosterAgent {
  kind: "agent";
  /** Author id: `agent:<name>`. */
  id: string;
  name: string;
  label: string;
  harness: string;
  status: string;
  /** The line under the name, or null for none. */
  caption: string | null;
  /** Asleep or paused: drawn quieter. */
  dim: boolean;
  sessions: number;
  /** Who can task, control and approve it (the defaults from a daemon without rights). */
  rights: AgentRights;
  /** What you may do with it (all true from a daemon without rights: you are its owner). */
  youCan: Record<AgentRight, boolean>;
  /** Who it is (null from a daemon without identities: no owner badge, no card). */
  identity: MemberIdentity | null;
  /** Another agent here uses the same name: its owner shows after the AI tag. */
  collides: boolean;
  /** What the Space calls it (`@claude`); `name` is its key on this machine (`claude-2` in a second Space). */
  handle: string;
}

/** How an author shows in the thread. */
export interface RosterAuthor {
  label: string;
  kind: "person" | "agent" | "system";
  harness: string;
  /** An agent's handle (`claude-2`). */
  handle?: string;
  identity?: MemberIdentity | null;
  /** Another agent here shares its name. */
  collides?: boolean;
  /** Its member, while it is still in the Space. */
  agent?: RosterAgent;
}

export interface RosterPerson {
  kind: "person";
  /** Author id when they posted, else `share:<who>`. */
  id: string;
  label: string;
  me: boolean;
}

export interface Roster {
  agents: RosterAgent[];
  people: RosterPerson[];
  /** How an author shows in the thread. */
  author(a: ThreadAuthor): RosterAuthor;
  /** Your account id, when known. */
  me: string | null;
  /** You are the Space's owner or an admin (a daemon that doesn't say your role: its owner). */
  admin: boolean;
  /** Whether you may remove this agent: the Space's owner and admins, and the agent's own owner. */
  canRemove(agent: RosterAgent | MemberIdentity): boolean;
  /** Names the composer's `@` picker offers (agents first). */
  candidates: MentionCandidate[];
  /** Lower-case handles that draw as mention chips. */
  handles: Set<string>;
}

/** An agent's name in the thread: its harness's name when the member is
 * named after the harness (`claude` running `claude-code` is "Claude Code"),
 * else its own name. */
export function agentLabel(name: string, harness: string, harnessName: (h: string) => string): string {
  const h = harness.toLowerCase();
  const n = name.toLowerCase();
  if (h && (n === h || h.startsWith(`${n}-`) || h.endsWith(`-${n}`))) return harnessName(harness);
  return capital(name);
}

const capital = (s: string) => (s ? s.charAt(0).toUpperCase() + s.slice(1) : s);

/** `grace@example.com` reads as Grace. */
export function personLabel(who: string): string {
  const local = who.includes("@") ? who.slice(0, who.indexOf("@")) : who;
  return capital(local.split(/[._+-]/)[0] || local);
}

const AGENT_CAPTION: Record<string, string> = { working: "Working", waiting: "Waiting", paused: "Paused", failed: "Failed" };

/** Where an agent posts from, under its name when it isn't busy: its computer, or the Space it runs in. */
export function postsFrom(identity: MemberIdentity): string {
  if (identity.device) return identity.device.name;
  return identity.origin === "space" ? "Runs in this Space" : "Connected";
}

/** The agents whose name another agent here also uses (by handle). The daemon's `collides` counts too. */
export function collidingNames(agents: readonly { name: string; label: string; identity: MemberIdentity | null }[]): Set<string> {
  const count = new Map<string, number>();
  const key = (a: { label: string }) => a.label.trim().toLowerCase();
  for (const a of agents) count.set(key(a), (count.get(key(a)) ?? 0) + 1);
  return new Set(agents.filter((a) => (count.get(key(a)) ?? 0) > 1 || a.identity?.collides).map((a) => a.name));
}

export function buildRoster(input: {
  members: readonly AgentMember[];
  shares: readonly ShareEntry[];
  entries: readonly ThreadEntry[];
  me: string | null;
  /** The signed-in account (`ada@example.com`), for "you" before you posted. */
  identity: string | null;
  harnessName: (harness: string) => string;
  /** Your role in the Space (null: a daemon that doesn't say, the owner's). */
  role?: ThreadRole | null;
}): Roster {
  const agents: RosterAgent[] = input.members.map((m) => ({
    kind: "agent",
    id: `agent:${m.name}`,
    name: m.name,
    handle: m.handle || m.name,
    // The name the agent chose; without an identity, the harness's name when it is named after it.
    label: m.identity?.displayName || agentLabel(m.name, m.harness, input.harnessName),
    harness: m.identity?.harness.id || m.harness,
    status: m.status,
    // What it says it is doing (`task #12: …`, `2 open tasks`), else the state in a word, else where it posts from.
    caption: m.activity ?? AGENT_CAPTION[m.status] ?? (m.identity ? postsFrom(m.identity) : null),
    dim: m.status === "asleep" || m.status === "paused",
    sessions: m.sessions,
    rights: m.rights ?? DEFAULT_AGENT_RIGHTS,
    youCan: m.youCan ?? { task: true, control: true, approve: true },
    identity: m.identity,
    collides: false,
  }));
  const colliding = collidingNames(agents);
  for (const a of agents) a.collides = colliding.has(a.name);
  const agentById = new Map(agents.map((a) => [a.id, a]));
  const role = input.role ?? null;

  // People: you first, then whoever it is shared with, then anyone else who posted.
  const people: RosterPerson[] = [];
  const seen = new Set<string>();
  const add = (p: RosterPerson, ...keys: string[]) => {
    if (keys.some((k) => seen.has(k))) return;
    keys.forEach((k) => seen.add(k));
    people.push(p);
  };
  const posted = new Map<string, ThreadAuthor>();
  for (const e of input.entries) if (e.author.kind === "person") posted.set(e.author.id, e.author);
  const mine = input.me ? posted.get(input.me) : undefined;
  const myLabel = mine?.name || (input.identity ? personLabel(input.identity) : "You");
  add({ kind: "person", id: input.me ?? "me", label: myLabel, me: true }, input.me ?? "me", myLabel.toLowerCase());
  for (const s of input.shares) {
    const label = personLabel(s.who);
    const author = [...posted.values()].find((a) => a.name.toLowerCase() === label.toLowerCase());
    add({ kind: "person", id: author?.id ?? `share:${s.who}`, label, me: false }, author?.id ?? `share:${s.who}`, label.toLowerCase());
  }
  for (const a of posted.values()) add({ kind: "person", id: a.id, label: a.name, me: false }, a.id, a.name.toLowerCase());

  const candidates: MentionCandidate[] = [
    ...agents.map((a) => ({ handle: a.handle, label: a.label, kind: "agent" as const })),
    ...people.filter((p) => !p.me).map((p) => ({ handle: p.label.toLowerCase().replace(/\s+/g, ""), label: p.label, kind: "person" as const })),
  ];
  const labels = new Set(agents.map((a) => a.label.trim().toLowerCase()));
  return {
    agents,
    people,
    candidates,
    handles: new Set(candidates.map((c) => c.handle.toLowerCase())),
    me: input.me,
    admin: role === null || role === "owner" || role === "admin",
    canRemove(agent) {
      const identity = "kind" in agent ? agent.identity : agent;
      if (role === null || role === "owner" || role === "admin") return true;
      return !!identity && !!input.me && identity.owner.id === input.me;
    },
    author(a) {
      if (a.kind === "agent") {
        const m = agentById.get(a.id);
        const handle = a.id.startsWith("agent:") ? a.id.slice("agent:".length) : a.name;
        // A member still here draws as it is now; one that left keeps how it posted.
        const identity = m?.identity ?? a.identity ?? null;
        const label = m?.label ?? (identity?.displayName || capital(a.name || handle));
        const collides = m ? m.collides : !!identity && (identity.collides || labels.has(label.trim().toLowerCase()));
        return { label, kind: "agent", harness: m?.harness ?? identity?.harness.id ?? "", handle, identity, collides, agent: m };
      }
      return { label: a.name || "Someone", kind: a.kind, harness: "" };
    },
  };
}
