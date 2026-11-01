// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { AgentMember } from "@/bridge";

import { buildRoster } from "./roster";

const member = (over: Partial<AgentMember>): AgentMember =>
  ({
  name: "claude",
  harness: "claude-code",
  status: "idle",
  activity: null,
  sessions: 1,
  rights: null,
  ...over,
  }) as AgentMember;

describe("a Space's @claude when another Space has the plain name", () => {
  it("answers to the handle the Space knows it by, not the key it is stored under", () => {
    const roster = buildRoster({
      members: [member({ name: "claude-2", handle: "claude" })],
      shares: [],
      entries: [],
      me: null,
      identity: null,
      harnessName: () => "Claude Code",
    });
    expect(roster.handles.has("claude")).toBe(true);
    expect(roster.handles.has("claude-2")).toBe(false);
    expect(roster.candidates.find((c) => c.kind === "agent")?.handle).toBe("claude");
    // Its key is still what ids and lookups use.
    expect(roster.agents[0]).toMatchObject({ id: "agent:claude-2", name: "claude-2", handle: "claude" });
  });

  it("is its own name when the Space has no alias", () => {
    const roster = buildRoster({ members: [member({})], shares: [], entries: [], me: null, identity: null, harnessName: () => "Claude Code" });
    expect(roster.agents[0]!.handle).toBe("claude");
  });
});
