// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it, vi } from "vitest";

import { agentSignInMethods } from "../src/bridge/agent-sign-in";

const ctx = {} as never;

describe("agents.computerSignIn", () => {
  it("calls the daemon's agent_sign_in with the agent's own sign-in action", async () => {
    const tool = vi.fn(async () => ({ state: "signing_in", url: "https://auth.openai.com/codex/device", code: "ABCD-12345", paste_code: false }));
    const m = agentSignInMethods(ctx, () => tool);
    await m["agents.computerSignIn"]!({ space: "mac-mini", agent: "openai-codex", action: "start" }, {});
    expect(tool).toHaveBeenCalledWith("agent_sign_in", { space: "mac-mini", agent: "openai-codex", action: "start" });
    await m["agents.computerSignIn"]!({ space: "mac-mini", agent: "claude-code", action: "code", code: " abc#def ", afterRefusal: true }, {});
    expect(tool).toHaveBeenLastCalledWith("agent_sign_in", { space: "mac-mini", agent: "claude-code", action: "code", code: "abc#def", after_refusal: true });
  });

  it("refuses other agents and actions before the daemon", async () => {
    const tool = vi.fn();
    const m = agentSignInMethods(ctx, () => tool);
    await expect(m["agents.computerSignIn"]!({ space: "s", agent: "hermes", action: "start" }, {})).rejects.toThrow();
    await expect(m["agents.computerSignIn"]!({ space: "s", agent: "claude-code", action: "copy" }, {})).rejects.toThrow();
    expect(tool).not.toHaveBeenCalled();
  });

  it("a daemon without the tool is unsupported", async () => {
    const m = agentSignInMethods(ctx, () => async () => Promise.reject(new Error("unknown tool agent_sign_in")));
    await expect(m["agents.computerSignIn"]!({ space: "s", agent: "claude-code", action: "status" }, {})).rejects.toMatchObject({ code: "unsupported" });
  });
});
