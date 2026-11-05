// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * From the account's relay directory to the computers and Spaces Home draws. Which row is a Space and which is a computer is
 * the app core's call (`directory.view`, the desktop's rule: a computer is never a Space); this file only pairs the core's
 * answer with the relay rows the calls need.
 */

import type { ComputerRow, DirectorySpace } from "../../core/views";
import type { MachineInfo, SpaceScope } from "../types";
import type { RelayMachine } from "./relay";

export interface SpaceMachine {
  /** The Space's own relay machine id. */
  machine: RelayMachine;
  /** The computer that runs it (its host); the Space itself when no computer is above it. */
  hostId: string;
  /** The Space's thread lives on `hostId`'s daemon (only a Space a computer runs has one the phone can reach). */
  threadOnHost: boolean;
  scope: SpaceScope;
  kindLabel: string;
  role: "owner" | "editor" | "viewer";
  /** The desktop's word for its state: Running, Stopped, Starting, Offline, Not sharing. */
  state: string;
}

const glyph = (k: ComputerRow["kind"]): MachineInfo["kind"] => (k === "windows" ? "pc" : k === "mac" || k === "linux" || k === "cloud" ? k : "mac");

/** The computers a call can go to (phones and unconfirmed computers are listed, never used) and the Spaces of the pool. */
export function mapDirectory(rows: RelayMachine[], computers: ComputerRow[], spaces: DirectorySpace[]): { machines: MachineInfo[]; spaces: SpaceMachine[] } {
  const byId = new Map(rows.map((r) => [r.id, r]));
  const usable = computers.filter((c) => !c.isPhone && !c.unconfirmed);
  const machines = new Map<string, MachineInfo>(usable.map((c) => [c.id, { id: c.id, name: c.name, kind: glyph(c.kind), online: c.online, lastSeenMs: null }]));
  const unconfirmed = new Set(computers.filter((c) => c.unconfirmed).map((c) => c.id));
  const out: SpaceMachine[] = [];
  for (const s of spaces) {
    const machine = byId.get(s.machine);
    if (!machine) continue;
    // Never a call to a computer nobody confirmed (a stolen token could have planted it), nor to a Space it runs.
    if (!machine.confirmed || (s.host && unconfirmed.has(s.host))) continue;
    if (s.host && !machines.has(s.host)) {
      // Shared with this account while its computer is not: the relay grants that Space's thread alone.
      machines.set(s.host, { id: s.host, name: s.hostName || "Shared with you", kind: "mac", online: true, lastSeenMs: null });
    }
    out.push({ machine, hostId: s.host || s.machine, threadOnHost: s.host !== "", scope: "machine", kindLabel: s.kindLabel, role: s.role, state: s.state });
  }
  return { machines: [...machines.values()], spaces: out };
}
