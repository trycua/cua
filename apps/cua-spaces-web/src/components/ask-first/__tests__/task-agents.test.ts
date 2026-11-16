// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// CUA-3193: Home offered @claude on a computer without Claude Code. Only agents the setup found
// on this computer are offered; with none, Home offers to install Claude Code.

import { describe, expect, it } from "vitest";

import type { AgentSetupRow } from "@/bridge";

import { chooseAgent, offeredAgents } from "../task-agents";
import { installedTaskAgents } from "../use-installed-agents";

const row = (agent: string, installed: boolean, configured = false) => ({ agent, name: agent, installed, configured }) as AgentSetupRow;

describe("who does a job on Home", () => {
  it("counts an agent only when it is on this computer, not when Cua only wrote its files", () => {
    // `configured` alone is Cua's own skills and MCP entry (CUA-3063): no Claude Code.
    const set = installedTaskAgents([row("claude-code", false, true), row("codex", true), row("cursor", true)]);
    expect([...set]).toEqual(["openai-codex"]);
  });

  it("offers only installed agents and never falls back to one that isn't", () => {
    const none = new Set<string>();
    expect(offeredAgents(none)).toEqual([]);
    expect(chooseAgent(null, null, none)).toBeNull();
    // A pick or a most-used agent that is gone from this computer is not offered either.
    expect(chooseAgent("claude-code", "claude-code", none)).toBeNull();
    const codex = new Set(["openai-codex"]);
    expect(chooseAgent(null, "claude-code", codex)).toBe("openai-codex");
    const both = new Set(["openai-codex", "claude-code"]);
    expect(offeredAgents(both).map((a) => a.id)).toEqual(["claude-code", "openai-codex"]);
    expect(chooseAgent(null, "openai-codex", both)).toBe("openai-codex");
    expect(chooseAgent("claude-code", "openai-codex", both)).toBe("claude-code");
  });
});
