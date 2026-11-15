// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * One Space row for Home: the status, the words and the order. The agent
 * words ("Running tests in checkout-api", "Waiting for you in design-review")
 * and the order of the agent list are the app core's (`agentList.view`); this
 * file only folds a Space's agents into the Space's one status.
 */

import { agentList, type AgentListRow, type AgentTone, type ListSpace } from "../core/views";
import type { AgentMember, MachineInfo, Scene, SpaceList, SpaceMemberRow, SpaceScope, SpaceSummary, ThreadEntry } from "./types";

export interface SpaceInput {
  id: string;
  name: string;
  machine: MachineInfo;
  scope: SpaceScope;
  kindLabel: string;
  scene: Scene;
  role: "owner" | "editor" | "viewer";
  members: AgentMember[];
  people: SpaceMemberRow[];
  last: ThreadEntry | null;
  unread: number;
  canSleep: boolean;
  canShare: boolean;
  /** The desktop's word for the Space's state when the directory says it is not running: Stopped, Offline, Starting. */
  state?: string;
}

export const STATUS_WORD: Record<AgentTone, string> = {
  you: "Needs you",
  working: "Running",
  asleep: "Asleep",
  idle: "Idle",
  failed: "Failed",
  stopped: "Stopped",
  creating: "Starting",
  "not-connected": "Not connected",
};

const RANK: Record<AgentTone, number> = { you: 0, "not-connected": 1, failed: 2, working: 3, creating: 4, idle: 5, asleep: 6, stopped: 7 };

export function listMembers(spaceId: string, spaceName: string, members: AgentMember[], lastMs: number, unread: number): ListSpace {
  return {
    spaceId,
    spaceName,
    members: members.map((m) => ({
      handle: m.name,
      harness: m.harness,
      displayName: m.identity?.displayName ?? "",
      ownerId: m.identity?.owner.id ?? "",
      status: m.status,
      activity: m.activity,
      unread: m.dm?.unread ?? unread,
      lastMs,
      needsMs: (m.needsYou ?? []).map((n) => n.tsMs),
      pending: m.identity?.state === "pending",
    })),
  };
}

/** The Space's one tone: the most urgent of its agents (an offline machine shows as stopped). */
export function spaceTone(rows: AgentListRow[], machineOnline: boolean): AgentTone {
  if (!machineOnline) return "stopped";
  if (!rows.length) return "idle";
  return rows.reduce((best, r) => (RANK[r.tone] < RANK[best] ? r.tone : best), rows[0]!.tone);
}

export function summarize(inputs: SpaceInput[], machines: MachineInfo[], loaded = true, incomplete = false): SpaceList {
  const listSpaces = inputs.map((s) => listMembers(s.id, s.name, s.members, s.last?.tsMs ?? 0, s.unread));
  const view = agentList(listSpaces);
  const rows = view?.rows ?? [];
  const spaces: SpaceSummary[] = inputs.map((s) => {
    const mine = rows.filter((r) => r.places.some((p) => p.spaceId === s.id));
    const tone = spaceTone(mine, s.machine.online);
    const top = mine.find((r) => r.tone === tone) ?? mine[0];
    return {
      id: s.id,
      name: s.name,
      machineId: s.machine.id,
      machineName: s.machine.name,
      machineOnline: s.machine.online,
      scope: s.scope,
      kindLabel: s.kindLabel,
      tone,
      statusWord: STATUS_WORD[tone],
      statusLine: s.state && s.state !== "Running" ? s.state : s.machine.online ? (top?.line ?? "") : "Offline",
      members: [
        ...s.members.map<SpaceMemberRow>((m) => ({
          id: m.name,
          name: m.identity?.displayName || m.name,
          kind: "agent",
          harness: m.harness,
          status: m.status,
          activity: m.activity,
          role: null,
          here: m.status === "working",
        })),
        ...s.people,
      ],
      lastMessage: s.last ? { text: s.last.text, author: s.last.author.name, tsMs: s.last.tsMs } : null,
      unread: s.unread,
      scene: s.scene,
      role: s.role,
      canSleep: s.canSleep,
      canShare: s.canShare,
    };
  });
  spaces.sort((a, b) => RANK[a.tone] - RANK[b.tone] || (b.lastMessage?.tsMs ?? 0) - (a.lastMessage?.tsMs ?? 0));
  return { spaces, machines, incomplete, reaching: incomplete ? machines.filter((m) => m.online).length : 0, agents: rows };
}
