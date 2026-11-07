// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { SYSTEM_BOT, isUnresolvedAgent } from "./roster";

describe("an author nobody stands behind is Cua's system author", () => {
  it("is one fixed system author, named Cua, that cannot pass for an agent", () => {
    expect(SYSTEM_BOT).toEqual({ kind: "system", id: "system:children", name: "Cua" });
  });

  it("a member, a departed agent that kept its identity, and a person are themselves; an unmatched agent is the bot", () => {
    expect(isUnresolvedAgent({ kind: "agent", agent: { handle: "claude" } as never })).toBe(false);
    expect(isUnresolvedAgent({ kind: "agent", identity: { handle: "codex" } as never })).toBe(false);
    expect(isUnresolvedAgent({ kind: "person" })).toBe(false);
    expect(isUnresolvedAgent({ kind: "system" })).toBe(false);
    expect(isUnresolvedAgent({ kind: "agent" })).toBe(true);
  });
});
