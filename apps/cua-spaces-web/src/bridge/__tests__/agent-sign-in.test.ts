// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { agentKeysFromWire } from "../ops/agent-keys";
import { AGENT_SIGN_IN_TOOLS, computerSignInAgentOf, computerSignInFromTool, isMissingSignInTool, webkitAgentSignInOps } from "../ops/agent-sign-in";

describe("an agent's own sign-in on another computer (CUA-3907)", () => {
  it("reads every state the daemon answers", () => {
    expect(computerSignInFromTool({ state: "not_signed_in" }).state).toBe("not_signed_in");
    expect(computerSignInFromTool({ state: "signing_in", url: "https://auth.openai.com/codex/device", code: "ABCD-12345", paste_code: false })).toEqual({
      state: "signing_in",
      url: "https://auth.openai.com/codex/device",
      code: "ABCD-12345",
      pasteCode: false,
      reason: null,
    });
    expect(computerSignInFromTool({ state: "signing_in", url: "https://claude.com/cai/oauth/authorize?x=1", code: null, paste_code: true })).toMatchObject({
      pasteCode: true,
      code: null,
    });
    expect(computerSignInFromTool({ state: "signed_in" }).state).toBe("signed_in");
    expect(computerSignInFromTool({ state: "failed", reason: "Codex's sign-in stopped: denied" })).toMatchObject({
      state: "failed",
      reason: "Codex's sign-in stopped: denied",
    });
    expect(computerSignInFromTool({ state: "expired" }).state).toBe("expired");
    expect(computerSignInFromTool({ state: "nonsense" }).state).toBe("not_signed_in");
  });

  it("shows only an https link", () => {
    expect(computerSignInFromTool({ state: "signing_in", url: "javascript:alert(1)" }).url).toBeNull();
    expect(computerSignInFromTool({ state: "signing_in", url: "http://example.com" }).url).toBeNull();
  });

  it("sends the daemon only what it takes", () => {
    const args = AGENT_SIGN_IN_TOOLS["agents.computerSignIn"].args;
    expect(args({ space: "mac-mini", agent: "openai-codex", action: "start" })).toEqual({ space: "mac-mini", agent: "openai-codex", action: "start" });
    expect(args({ space: "mac-mini", agent: "claude-code", action: "code", code: " a#b ", afterRefusal: true })).toEqual({
      space: "mac-mini",
      agent: "claude-code",
      action: "code",
      code: "a#b",
      after_refusal: true,
    });
  });

  it("is unsupported on a daemon without the tool", async () => {
    expect(isMissingSignInTool(new Error("unknown tool agent_sign_in"))).toBe(true);
    const ops = webkitAgentSignInOps(async () => Promise.reject(new Error("unknown tool agent_sign_in")));
    await expect(ops["agents.computerSignIn"]({ space: "s", agent: "claude-code", action: "status" })).rejects.toMatchObject({
      name: "UnsupportedOperationError",
    });
  });

  it("only Claude Code and Codex sign in on the computer", () => {
    expect(computerSignInAgentOf("claude-code")).toBe("claude-code");
    expect(computerSignInAgentOf("codex")).toBe("openai-codex");
    expect(computerSignInAgentOf("hermes")).toBeNull();
  });

  it("copying the sign-in reads as off unless the daemon says on", () => {
    expect(agentKeysFromWire({ keys: [], copy_sign_in: false }).copySignIn).toBe(false);
    expect(agentKeysFromWire({ keys: [], copy_sign_in: true }).copySignIn).toBe(true);
    expect(agentKeysFromWire({ keys: [] }).copySignIn).toBeUndefined();
  });
});
