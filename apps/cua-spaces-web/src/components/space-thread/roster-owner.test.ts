// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { buildRoster } from "./roster";

const base = { members: [], entries: [], me: null, identity: "guest@example.com", harnessName: () => "" };

describe("who is in a shared Space (CUA-3040)", () => {
  it("a guest sees the owner next to themselves, though only the owner can list the shares", () => {
    const roster = buildRoster({ ...base, shares: [], owner: { name: "Ada", email: "ada@example.com" } });
    expect(roster.people.map((p) => [p.label, p.me])).toEqual([
      ["Guest", true],
      ["Ada", false],
    ]);
  });

  it("the owner is listed once when the share list names them too", () => {
    const roster = buildRoster({ ...base, shares: [{ who: "ada@example.com", role: "editor" }], owner: { name: "Ada", email: "ada@example.com" } });
    expect(roster.people.filter((p) => p.label === "Ada")).toHaveLength(1);
  });

  it("the owner sees whoever the Space is shared with", () => {
    const roster = buildRoster({ ...base, identity: "ada@example.com", shares: [{ who: "bob@example.com", role: "viewer" }] });
    expect(roster.people.map((p) => p.label)).toEqual(["Ada", "Bob"]);
  });
});
