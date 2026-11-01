// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The line under an idle connected agent: where it posts from while it watches
// the thread, "Waits for you to message it" when it does not.

import { describe, expect, it } from "vitest";

import type { MemberIdentity } from "@/bridge";

import { idleCaption } from "./roster";

const NOW = 1_000_000_000;
const id = (over: Partial<MemberIdentity> = {}): MemberIdentity => ({
  origin: "external",
  handle: "codex",
  displayName: "Codex",
  collides: false,
  icon: "harness",
  harness: { id: "codex", version: null, reported: true },
  owner: { id: "acct_ada", name: "Ada" },
  device: { id: "dev", name: "Ada's MacBook Pro" },
  joined: { method: "prompt", atMs: 0, by: null },
  state: "active",
  lastSeenMs: NOW - 10_000,
  ...over,
});

describe("an idle connected agent's line", () => {
  it("says where it posts from while it is watching the thread", () => {
    expect(idleCaption(id(), NOW)).toBe("Ada's MacBook Pro");
  });

  it("says it waits for you when it has not asked for its inbox in a while, or never", () => {
    expect(idleCaption(id({ lastSeenMs: NOW - 10 * 60_000 }), NOW)).toBe("Waits for you to message it");
    expect(idleCaption(id({ lastSeenMs: null }), NOW)).toBe("Waits for you to message it");
  });

  it("leaves a started agent and one waiting to be let in as they are", () => {
    expect(idleCaption(id({ origin: "space", device: null, lastSeenMs: null }), NOW)).toBe("Runs in this Space");
    expect(idleCaption(id({ state: "pending", lastSeenMs: null }), NOW)).toBe("Ada's MacBook Pro");
  });
});
