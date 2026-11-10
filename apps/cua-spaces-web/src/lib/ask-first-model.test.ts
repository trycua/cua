// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { beforeEach, describe, expect, it } from "vitest";

import type { AgentAccount, AgentGroup, AgentsView } from "@/bridge";

import { agentLine, pickAccount, pickedAgent, pickOf, rememberAgent, resetPickedAgent, resolveAgent } from "./ask-first-model";

const account = (line: string, extra: Partial<AgentAccount> = {}): AgentAccount => ({
  provider: "anthropic",
  providerLabel: "Anthropic",
  icon: "anthropic",
  env: "CLAUDE_CODE_OAUTH_TOKEN",
  slot: null,
  line,
  via: "sign-in",
  selected: false,
  models: [
    { id: "haiku", label: "Claude Haiku 5.5", fast: true, default: false },
    { id: "sonnet", label: "Claude Sonnet 5.5", fast: false, default: true },
    { id: "haiku-old", label: "Claude Haiku 4.5", fast: true, default: false, legacy: true },
  ],
  defaultModel: "sonnet",
  ...extra,
});
const group = (app: string, accounts: AgentAccount[], extra: Partial<AgentGroup> = {}): AgentGroup => ({
  harness: app === "codex" ? "openai-codex" : app,
  app,
  label: app === "codex" ? "OpenAI Codex" : app === "claude-code" ? "Claude Code" : app,
  state: accounts.length ? "connected" : "not-connected",
  accounts,
  signInLabel: "Sign in",
  ...extra,
});
const claude = group("claude-code", [account("personal", { selected: true }), account("work Max", { slot: "work" }), account("side Max", { slot: "side" })]);
const view = (groups: AgentGroup[]): AgentsView => ({
  groups,
  default: null,
  empty: null,
});

beforeEach(() => {
  localStorage.clear();
  resetPickedAgent();
});

describe("the agent the box starts with", () => {
  it("is the first connected app with its own account and quick model when nothing was picked", () => {
    const v = {
      ...view([group("codex", []), claude]),
      default: { harness: "claude-code", provider: "anthropic", env: "CLAUDE_CODE_OAUTH_TOKEN", slot: null, model: "sonnet" },
    };
    const s = resolveAgent(v, null)!;
    expect(s.harness).toBe("claude-code");
    expect(agentLine(s)).toBe("Claude Code");
    expect(s.model.id).toBe("sonnet");
  });

  it("keeps a saved older model id the person chose before the default moved", () => {
    const s = resolveAgent(view([claude]), { harness: "claude-code", env: "CLAUDE_CODE_OAUTH_TOKEN", model: "haiku-old" })!;
    expect(s.model.id).toBe("haiku-old");
  });

  it("is the last pick while that app and account are still there, with its model", () => {
    const s = resolveAgent(view([claude]), { harness: "claude-code", env: "CLAUDE_CODE_OAUTH_TOKEN", slot: "work", model: "sonnet" })!;
    expect(agentLine(s)).toBe("Claude Code");
    expect(s.account.line).toBe("work Max");
    expect(s.model.id).toBe("sonnet");
  });

  it("never uses a remembered account that is gone: the app's own choice stands in", () => {
    const s = resolveAgent(view([claude]), { harness: "claude-code", env: "CLAUDE_CODE_OAUTH_TOKEN", slot: "removed", model: "sonnet" })!;
    expect(agentLine(s)).toBe("Claude Code");
    expect(s.account.line).toBe("personal");
    // The model it remembered is kept while the account offers it.
    expect(s.model.id).toBe("sonnet");
  });

  it("falls back to the default when the remembered app is no longer connected, and to nothing when none is", () => {
    const v = {
      ...view([group("codex", []), claude]),
      default: { harness: "claude-code", provider: "anthropic", env: "CLAUDE_CODE_OAUTH_TOKEN", slot: null, model: "sonnet" },
    };
    expect(resolveAgent(v, { harness: "openai-codex", env: "CODEX_AUTH_JSON" })!.harness).toBe("claude-code");
    expect(resolveAgent(view([group("codex", [])]), null)).toBeNull();
    expect(resolveAgent(null, null)).toBeNull();
  });

  it("picking an account starts with its quick model, or the last model while it offers it", () => {
    const work = claude.accounts[1]!;
    expect(pickAccount(claude, work, null)).toEqual({ harness: "claude-code", env: "CLAUDE_CODE_OAUTH_TOKEN", slot: "work", model: "sonnet" });
    expect(pickAccount(claude, work, { harness: "claude-code", model: "haiku" }).model).toBe("haiku");
    expect(pickAccount(claude, work, { harness: "claude-code", model: "haiku-old" }).model).toBe("haiku-old");
    expect(pickAccount(claude, work, { harness: "openai-codex", model: "haiku" }).model).toBe("sonnet");
  });
});

describe("the last pick, kept on this computer", () => {
  it("is read back, and a malformed one is ignored", () => {
    expect(pickedAgent()).toBeNull();
    const s = resolveAgent(view([claude]), { harness: "claude-code", env: "CLAUDE_CODE_OAUTH_TOKEN", slot: "side", model: "haiku" })!;
    rememberAgent(pickOf(s));
    resetPickedAgent();
    expect(pickedAgent()).toEqual({ harness: "claude-code", model: "haiku", env: "CLAUDE_CODE_OAUTH_TOKEN", slot: "side" });
    localStorage.setItem("cua.askFirst.agent", "{not json");
    resetPickedAgent();
    expect(pickedAgent()).toBeNull();
  });
});

describe("the reasoning level of a pick", () => {
  const sonnet = group("claude-code", [
    account("p", {
      selected: true,
      models: [{ id: "claude-sonnet-5-5", label: "Claude Sonnet 5.5", fast: false, default: true }],
      defaultModel: "claude-sonnet-5-5",
    }),
  ]);
  it("is the model's own until one is chosen, and is remembered with the pick", () => {
    const v = view([sonnet]);
    expect(resolveAgent(v, { harness: "claude-code", env: "CLAUDE_CODE_OAUTH_TOKEN" })!.effort).toBe("high");
    const s = resolveAgent(v, { harness: "claude-code", env: "CLAUDE_CODE_OAUTH_TOKEN", effort: "low" })!;
    expect(s.effort).toBe("low");
    expect(pickOf(s).effort).toBe("low");
  });
  it("is none for a model without levels", () => {
    expect(resolveAgent(view([claude]), { harness: "claude-code", env: "CLAUDE_CODE_OAUTH_TOKEN", effort: "low" })!.effort).toBeNull();
  });
});
