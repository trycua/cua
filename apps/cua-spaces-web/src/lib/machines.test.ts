// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { Machine } from "@/bridge";
import { connectionLabel, machineOs, machinesSummary, machineSubtitle, presenceLabel, presenceWord, reachable, selectedMachine, spaceCount } from "./machines";

const NOW = Date.parse("2026-10-03T12:00:00Z");

const machine = (over: Partial<Machine>): Machine => ({
  id: "m",
  name: "M",
  os: "linux",
  via: "relay",
  connection: "relay",
  online: true,
  current: false,
  spaceIds: [],
  limits: [],
  ...over,
});

describe("machines", () => {
  it("counts machines in the Machines header in the singular too", () => {
    expect(machinesSummary(1, 1)).toBe("1 machine, 1 online");
    expect(machinesSummary(3, 2)).toBe("3 machines, 2 online");
  });

  it("reads the OS the host reported", () => {
    expect(machineOs(machine({ os: "macos" }))).toBe("macos");
    expect(machineOs(machine({ os: "Windows" }))).toBe("windows");
    expect(machineOs(machine({ os: "" }))).toBeNull();
  });

  it("says how a machine is reached", () => {
    expect(connectionLabel(machine({}))).toBe("Cua relay");
    expect(connectionLabel(machine({ connection: "direct" }))).toBe("Direct");
    expect(connectionLabel(machine({ connection: null, current: true }))).toBe("Not set up for access");
    expect(machineSubtitle(machine({ model: "Mac mini" }))).toBe("Mac mini · Cua relay");
    expect(machineSubtitle(machine({ os: "windows", connection: "direct" }))).toBe("Windows · Direct");
  });

  it("says when an offline machine was last seen", () => {
    expect(presenceLabel(machine({}), NOW)).toBe("Online");
    expect(presenceLabel(machine({ online: false }), NOW)).toBe("Offline");
    expect(presenceLabel(machine({ online: false, lastSeen: (NOW - 2 * 86_400_000) / 1000 }), NOW)).toBe("Offline, last seen 2 days ago");
  });

  it("says a machine its owner stopped sharing is not reachable", () => {
    const stopped = machine({ notSharing: "Studio stopped sharing" });
    // As the SwiftUI detail says it: the core's word, not "online".
    expect(presenceLabel(stopped, NOW)).toBe("Not reachable");
    expect(presenceWord(stopped)).toBe("Not reachable");
    expect(presenceLabel(stopped, NOW, "Unreachable here")).toBe("Unreachable here");
    expect(reachable(stopped)).toBe(false);
    expect(reachable(machine({}))).toBe(true);
    expect(presenceWord(machine({}))).toBe("Online");
    // Offline wins: it says nothing about sharing then.
    expect(presenceWord(machine({ online: false, notSharing: "x" }))).toBe("Offline");
    expect(presenceLabel(machine({ online: false, notSharing: "x" }), NOW)).toBe("Offline");
  });

  it("counts Spaces and keeps a valid selection", () => {
    expect([0, 1, 3].map(spaceCount)).toEqual(["No Spaces", "1 Space", "3 Spaces"]);
    const list = [machine({ id: "a" }), machine({ id: "b" })];
    expect(selectedMachine(list, "b")?.id).toBe("b");
    expect(selectedMachine(list, "gone")?.id).toBe("a");
    expect(selectedMachine([], null)).toBeUndefined();
  });
});
