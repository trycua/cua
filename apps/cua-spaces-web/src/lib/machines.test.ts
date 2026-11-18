// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { Machine } from "@/bridge";
import {
  chatComputers,
  connectionLabel,
  machineOs,
  machineSubtitle,
  presenceLabel,
  presenceWord,
  reachable,
  spaceCount,
  THIS_COMPUTER,
  computerFor,
  spacesLabel,
  thisComputerSubtitle,
} from "./machines";

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
  it("reads the OS the host reported", () => {
    expect(machineOs(machine({ os: "macos" }))).toBe("macos");
    expect(machineOs(machine({ os: "Windows" }))).toBe("windows");
    expect(machineOs(machine({ os: "" }))).toBeNull();
  });

  it("says how a machine is reached", () => {
    expect(connectionLabel(machine({}))).toBe("Cua relay");
    expect(connectionLabel(machine({ connection: "direct" }))).toBe("Direct");
    expect(connectionLabel(machine({ connection: null, current: true }))).toBe("Not set up for access");
    expect(machineSubtitle(machine({ model: "Mac mini" }))).toBe("Mac mini, Cua relay");
    expect(machineSubtitle(machine({ os: "windows", connection: "direct" }))).toBe("Windows, Direct");
  });

  it("labels a signed-in machine apart from a machine's connection", () => {
    expect(connectionLabel(machine({ connection: null, device: true }))).toBe("Signed in, not sharing");
    expect(machineSubtitle(machine({ os: "windows", connection: null, device: true }))).toBe("Windows, Signed in, not sharing");
    expect(connectionLabel(machine({ connection: "relay", device: true }))).toBe("Cua relay");
    expect(connectionLabel(machine({ connection: null, current: true }))).toBe("Not set up for access");
    expect(connectionLabel(machine({ connection: null, current: false }))).toBe("Unknown");
  });

  it("says when an offline machine was last seen", () => {
    expect(presenceLabel(machine({}), NOW)).toBe("Online");
    expect(presenceLabel(machine({ online: false }), NOW)).toBe("Offline");
    expect(presenceLabel(machine({ online: false, lastSeen: (NOW - 2 * 86_400_000) / 1000 }), NOW)).toBe("Offline, last seen 2 days ago");
  });

  it("says a machine its owner stopped sharing is not sharing", () => {
    const stopped = machine({ notSharing: "Studio stopped sharing" });
    // The core's word (`machines::NOT_SHARING`), as the SwiftUI list says it.
    expect(presenceLabel(stopped, NOW)).toBe("Not sharing");
    expect(presenceWord(stopped)).toBe("Not sharing");
    expect(reachable(stopped)).toBe(false);
    expect(reachable(machine({}))).toBe(true);
    expect(presenceWord(machine({}))).toBe("Online");
    // Offline wins: it says nothing about sharing then.
    expect(presenceWord(machine({ online: false, notSharing: "x" }))).toBe("Offline");
    expect(presenceLabel(machine({ online: false, notSharing: "x" }), NOW)).toBe("Offline");
  });

  it("shows the core's status word and Spaces count when the host has them", () => {
    // CUA-1617: never a Space's "Suspended" or "Not reachable".
    const row = machine({ notSharing: "x", status: "Not sharing", spacesLabel: "No Spaces" });
    expect(presenceWord(row)).toBe("Not sharing");
    expect(presenceLabel(row, NOW)).toBe("Not sharing");
    expect(spacesLabel(row, 4)).toBe("No Spaces");
    expect(spacesLabel(machine({ device: true, spacesLabel: "" }), 0)).toBe("");
    expect(spacesLabel(machine({}), 1)).toBe("1 Space");
  });

  it("counts a shared desktop in the words (machines::desktop_spaces_label)", () => {
    expect(spacesLabel(machine({}), 0, true)).toBe("Desktop shared");
    expect(spacesLabel(machine({}), 2, true)).toBe("Desktop and 2 Spaces");
    // A label the core sent still wins.
    expect(spacesLabel(machine({ spacesLabel: "No Spaces" }), 0, true)).toBe("No Spaces");
    expect(spacesLabel(machine({ device: true, spacesLabel: "" }), 0, true)).toBe("");
  });

  it("counts Spaces and finds a computer for its page", () => {
    expect([0, 1, 3].map(spaceCount)).toEqual(["No Spaces", "1 Space", "3 Spaces"]);
    // A computer's page finds it by its id, and this one by `this-mac` whatever its own id.
    const mine = [
      { id: "a", current: false },
      { id: "mac-7", current: true },
    ];
    expect(computerFor(mine, "a")?.id).toBe("a");
    expect(computerFor(mine, THIS_COMPUTER)?.id).toBe("mac-7");
    expect(computerFor(mine, "gone")).toBeUndefined();
  });
});

describe("chatComputers", () => {
  const m = (over: Partial<Machine>): Machine => ({
    id: "m",
    name: "M",
    os: "macos",
    via: "relay",
    connection: "relay",
    online: true,
    current: false,
    spaceIds: [],
    limits: [],
    ...over,
  });

  it("keeps your other online computers once each, and leaves out this one, devices and offline ones", () => {
    const rows = [
      m({ id: "here", name: "This Mac", current: true }),
      m({ id: "mini", name: "Mac mini" }),
      m({ id: "mini", name: "Mac mini" }),
      m({ id: "web-1", name: "Web browser", os: "web", device: true }),
      m({ id: "web-2", name: "Web browser", os: "web", device: true }),
      m({ id: "old", name: "Old box", online: false }),
      m({ id: "off", name: "Not shared", notSharing: "Stopped sharing" }),
      m({ id: "pc", name: "Studio PC", os: "windows" }),
    ];
    const { computers, offline } = chatComputers(rows);
    expect(computers.map((c) => c.id)).toEqual(["mini", "pc"]);
    expect(offline).toBe(2);
  });
});

describe("thisComputerSubtitle", () => {
  it("says what this computer is, never its title again (CUA-3974)", () => {
    expect(thisComputerSubtitle([{ os: "macos", model: "MacBook Pro", current: true }], "This Mac")).toBe("MacBook Pro");
    expect(thisComputerSubtitle([{ os: "macos", current: true }], "This Mac")).toBe("macOS");
    expect(thisComputerSubtitle([{ os: "macos", model: "This Mac", current: true }], "This Mac")).toBeNull();
    expect(thisComputerSubtitle([{ os: "linux", model: "Studio", current: false }], "This Mac")).toBeNull();
    expect(thisComputerSubtitle(undefined, "This Mac")).toBeNull();
  });
});
