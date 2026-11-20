// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Signing Claude Code or Codex in on another computer, the agent's own way
 * (CUA-3907). Cua does not copy your sign-in to other computers by default:
 * the computer's own `claude` / `codex` signs itself in, and Cua shows you the
 * link (and code) to finish it in your own browser. One operation, the
 * daemon's person-only tool:
 *
 * | Operation | Daemon tool | SwiftUI / Electron host | Tauri |
 * |---|---|---|---|
 * | `agents.computerSignIn {space, agent, action, code?, afterRefusal?}` | `agent_sign_in` | the tool's JSON as-is | unsupported |
 */

import { isUnsupported, UnsupportedOperationError } from "../adapter";
import type { OpCoverage } from "../coverage";

/** The agents that sign in on the computer they run on (harness ids). */
export type ComputerSignInAgent = "claude-code" | "openai-codex";
export const COMPUTER_SIGN_IN_AGENTS: readonly ComputerSignInAgent[] = ["claude-code", "openai-codex"];

/** `status` reads where it stands; `start` begins it; `code` hands over the pasted code (Claude Code); `cancel` stops it. */
export type ComputerSignInAction = "status" | "start" | "code" | "cancel";

/** Where a computer's sign-in stands for one agent (`cua_agents::remote_login::LoginState`). */
export type ComputerSignInStep = "not_signed_in" | "signing_in" | "signed_in" | "failed" | "expired";
export const COMPUTER_SIGN_IN_STEPS: readonly ComputerSignInStep[] = ["not_signed_in", "signing_in", "signed_in", "failed", "expired"];

export interface ComputerSignInState {
  state: ComputerSignInStep;
  /** `signing_in`: the page to open (null while the agent is still starting). */
  url: string | null;
  /** `signing_in`, Codex: the code to type on that page. */
  code: string | null;
  /** `signing_in`, Claude Code: paste the code the page shows back here. */
  pasteCode: boolean;
  /** `failed`: why, in plain words. */
  reason: string | null;
}

export interface AgentSignInOps {
  "agents.computerSignIn": {
    args: { space: string; agent: ComputerSignInAgent; action: ComputerSignInAction; code?: string; afterRefusal?: boolean };
    result: ComputerSignInState;
  };
}

export type AgentSignInOpName = keyof AgentSignInOps;

export const AGENT_SIGN_IN_OPERATIONS = ["agents.computerSignIn"] as const satisfies readonly AgentSignInOpName[];
export const AGENT_SIGN_IN_WEBKIT_METHODS = AGENT_SIGN_IN_OPERATIONS;

const TAURI = "an agent's sign-in on another computer is started by the cua daemon's person-only tool, which the Tauri shell does not call";
export const AGENT_SIGN_IN_COVERAGE = {
  "agents.computerSignIn": { webkit: { methods: ["agents.computerSignIn"] }, tauri: [], unsupported: { tauri: TAURI } },
} as const satisfies Record<AgentSignInOpName, OpCoverage>;

/** The daemon tool and its arguments (the native hosts use this mapping). */
export const AGENT_SIGN_IN_TOOLS = {
  "agents.computerSignIn": {
    tool: "agent_sign_in",
    args: (a: AgentSignInOps["agents.computerSignIn"]["args"]) => ({
      space: a.space,
      agent: a.agent,
      action: a.action,
      ...(a.action === "code" && a.code ? { code: a.code.trim() } : {}),
      ...(a.afterRefusal ? { after_refusal: true } : {}),
    }),
  },
} as const;

/** Arguments that take it down its usual path (coverage tests). */
export const AGENT_SIGN_IN_ARGS = {
  "agents.computerSignIn": { space: "mac-mini", agent: "openai-codex" as ComputerSignInAgent, action: "status" as ComputerSignInAction },
};

const rec = (v: unknown): Record<string, unknown> => (v && typeof v === "object" ? (v as Record<string, unknown>) : {});
/** A short plain line, else null. */
const line = (v: unknown, max = 512): string | null => (typeof v === "string" && v.trim() && v.length <= max && !/[\u0000-\u001f]/.test(v) ? v.trim() : null);
/** Only an https link is shown or opened. */
const link = (v: unknown): string | null => {
  const s = line(v, 4096);
  return s && /^https:\/\/[^\s]+$/.test(s) ? s : null;
};

/** `agent_sign_in`'s answer (snake_case) as the page reads it; anything unknown reads as not signed in. */
export function computerSignInFromTool(v: unknown): ComputerSignInState {
  const o = rec(v);
  const base: ComputerSignInState = { state: "not_signed_in", url: null, code: null, pasteCode: false, reason: null };
  switch (o.state) {
    case "signing_in":
      return { ...base, state: "signing_in", url: link(o.url), code: line(o.code, 32), pasteCode: o.paste_code === true };
    case "failed":
      return { ...base, state: "failed", reason: line(o.reason) ?? "The sign-in stopped. Try again." };
    case "signed_in":
    case "expired":
      return { ...base, state: o.state };
    default:
      return base;
  }
}

/** A daemon from before the tool: its "unknown tool" is `unsupported`. */
export function isMissingSignInTool(e: unknown): boolean {
  const m = (e instanceof Error ? e.message : String(e)).toLowerCase();
  return m.includes("spaces tool agent_sign_in") || ((m.includes("unknown tool") || m.includes("no such tool")) && m.includes("agent_sign_in"));
}

type Request = <T>(method: AgentSignInOpName, args?: Record<string, unknown>) => Promise<T>;

/** The SwiftUI and Electron hosts: the host method, then this file's mapping. */
export function webkitAgentSignInOps(request: Request) {
  return {
    "agents.computerSignIn": async (a: AgentSignInOps["agents.computerSignIn"]["args"]) => {
      try {
        return computerSignInFromTool(await request<unknown>("agents.computerSignIn", { ...a }));
      } catch (e) {
        if (!isUnsupported(e) && isMissingSignInTool(e)) throw new UnsupportedOperationError("webkit", "agents.computerSignIn");
        throw e;
      }
    },
  };
}

/** The harness ids whose sign-in is made on the computer (`openai-codex`; `codex` is its setup id). */
export function computerSignInAgentOf(harness: string): ComputerSignInAgent | null {
  if (harness === "codex") return "openai-codex";
  return (COMPUTER_SIGN_IN_AGENTS as readonly string[]).includes(harness) ? (harness as ComputerSignInAgent) : null;
}
