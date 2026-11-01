// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * An agent's rights in words (CUA-2156): who can give it tasks, stop it
 * and answer its approvals. Short labels for the member's rows; the daemon
 * decides, this only says it.
 */

import { DEFAULT_AGENT_RIGHTS, type AgentRight, type AgentRights, type RightHolders, type ThreadRole } from "@/bridge";

export const RIGHT_ROWS: readonly { right: AgentRight; label: string }[] = [
  { right: "task", label: "Can task" },
  { right: "control", label: "Can stop" },
  { right: "approve", label: "Can approve" },
];

export type HolderChoice = "owner" | "admins" | "members" | "named";

/** The choice a row's menu shows for `h`. */
export const holderChoice = (h: RightHolders): HolderChoice => (Array.isArray(h) ? "named" : h);

/** `Owner`, `Admins`, `Members`, or `2 people` for named accounts. */
export function holdersLabel(h: RightHolders): string {
  if (Array.isArray(h)) return h.length === 1 ? "1 person" : `${h.length} people`;
  return h === "owner" ? "Owner" : h === "admins" ? "Admins" : "Members";
}

/** Who an open approval waits for, when you can't answer it. */
export function waitingFor(h: RightHolders): string {
  if (Array.isArray(h)) return "Waiting for an approver";
  return h === "owner" ? "Waiting for the owner" : h === "admins" ? "Waiting for the owner or an admin" : "Waiting for a member";
}

/** What a guest's composer says. */
export const GUEST_COMPOSER = "View only. You can read this thread but not post.";

/** Two holders of a right are the same: one group, or the same accounts. */
export function sameHolders(a: RightHolders, b: RightHolders): boolean {
  if (Array.isArray(a) || Array.isArray(b)) return Array.isArray(a) && Array.isArray(b) && a.length === b.length && a.every((x) => b.includes(x));
  return a === b;
}

/** Every right is at its default (task, stop, approve): the card shows one "Default permissions" line. */
export const rightsAreDefault = (rights: AgentRights): boolean => RIGHT_ROWS.every(({ right }) => sameHolders(rights[right], DEFAULT_AGENT_RIGHTS[right]));

/**
 * Who may change an agent's rights: the Space's owner and admins (a daemon
 * that does not say your role: its owner), and the agent's own owner when
 * they are a member. A guest or an agent never. The daemon decides; this
 * only keeps the card from offering what it would refuse.
 */
export function canEditRights(role: ThreadRole | null, ownsAgent: boolean): boolean {
  if (role === null || role === "owner" || role === "admin") return true;
  return role === "member" && ownsAgent;
}
