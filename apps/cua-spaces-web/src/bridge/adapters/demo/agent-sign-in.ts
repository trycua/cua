// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo's sign-in on another computer (CUA-3907), as the daemon's
 * `agent_sign_in` answers it: no computer but This Mac is signed in at first;
 * `start` shows a link (and Codex's code); Codex finishes on its own a few
 * seconds later, as if the code was typed on the page; Claude Code finishes
 * when a code is pasted (`bad` fails, as a wrong code would).
 */

import { HostError } from "../../adapter";
import type { AgentSignInOps, ComputerSignInState } from "../../ops/agent-sign-in";

const base: ComputerSignInState = { state: "not_signed_in", url: null, code: null, pasteCode: false, reason: null };

export function demoAgentSignIn(now: () => number) {
  const going = new Map<string, { startedMs: number; failed?: string }>();
  const signedIn = new Set<string>();
  return {
    "agents.computerSignIn": ({ space, agent, action, code, afterRefusal }: AgentSignInOps["agents.computerSignIn"]["args"]): ComputerSignInState => {
      if (space === "this-mac") throw new HostError("This Mac uses its own sign-in", "failed");
      const key = `${space}|${agent}`;
      const codex = agent === "openai-codex";
      if (action === "cancel") going.delete(key);
      if (action === "start") {
        signedIn.delete(key);
        going.set(key, { startedMs: now() });
      }
      if (action === "code") {
        const g = going.get(key);
        if (!g) throw new HostError("No sign-in is going. Start again.", "failed");
        if ((code ?? "").trim() === "bad") g.failed = "Claude Code's sign-in stopped: OAuth error: invalid code";
        else {
          going.delete(key);
          signedIn.add(key);
        }
      }
      const g = going.get(key);
      if (g?.failed) return { ...base, state: "failed", reason: g.failed };
      if (g && codex && now() - g.startedMs > 6000) {
        going.delete(key);
        signedIn.add(key);
      }
      if (going.has(key))
        return codex
          ? { ...base, state: "signing_in", url: "https://auth.openai.com/codex/device", code: "QX7M-4RT2K" }
          : { ...base, state: "signing_in", url: "https://claude.com/cai/oauth/authorize?code=true&state=demo", pasteCode: true };
      if (signedIn.has(key)) return { ...base, state: "signed_in" };
      return { ...base, state: afterRefusal && space === "demo-expired" ? "expired" : "not_signed_in" };
    },
  };
}
