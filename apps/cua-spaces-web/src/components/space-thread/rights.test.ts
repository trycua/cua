// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Who may change an agent's rights, and when they are all the defaults.

import { describe, expect, it } from "vitest";

import { DEFAULT_AGENT_RIGHTS } from "@/bridge";

import { canEditRights, rightsAreDefault, sameHolders } from "./rights";

describe("editing an agent's rights", () => {
  it("is offered to the Space's owner and admins, and to the agent's own owner, never to a guest", () => {
    expect(canEditRights("owner", false)).toBe(true);
    expect(canEditRights("admin", false)).toBe(true);
    // A daemon that does not say your role: you are its owner.
    expect(canEditRights(null, false)).toBe(true);
    expect(canEditRights("member", true)).toBe(true);
    expect(canEditRights("member", false)).toBe(false);
    expect(canEditRights("guest", true)).toBe(false);
    expect(canEditRights("agent", true)).toBe(false);
  });

  it("reads as default permissions only while all three rows are the defaults", () => {
    expect(rightsAreDefault({ ...DEFAULT_AGENT_RIGHTS })).toBe(true);
    expect(rightsAreDefault({ ...DEFAULT_AGENT_RIGHTS, task: "owner" })).toBe(false);
    expect(rightsAreDefault({ ...DEFAULT_AGENT_RIGHTS, approve: ["acct_ada"] })).toBe(false);
    expect(sameHolders(["a", "b"], ["b", "a"])).toBe(true);
    expect(sameHolders(["a"], "owner")).toBe(false);
  });
});
