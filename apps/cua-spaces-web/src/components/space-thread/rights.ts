// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * An agent's rights in words (CUA-2156): who can give it tasks, stop it
 * and answer its approvals. Short labels for the member's rows; the daemon
 * decides, this only says it.
 */

import type { AgentRight, RightHolders } from "@/bridge";

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
