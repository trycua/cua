// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Connected apps: which coding apps Cua is set up in, and the rules for
 * setting up newly installed ones. Pure: the hook in `hooks/use-connected-apps`
 * reads the apps and writes the state; this decides.
 */

import type { AgentSetupRow } from "@/bridge";

/** What Cua remembers between runs (one local key, `cua.connectedApps`). */
export interface ConnectedAppsState {
  /** Apps seen installed so far: only an app not in here is "new". Null before the first read. */
  known: string[] | null;
  /** Apps the person turned off: never set up again on its own. */
  off: string[];
  /**
   * The person said yes to setting up apps they install later (onboarding's
   * Set up, or the switch in Settings). Null: never asked, so Cua sets up
   * nothing on its own.
   */
  auto: boolean | null;
}

export const EMPTY_STATE: ConnectedAppsState = { known: null, off: [], auto: null };

export const STATE_KEY = "cua.connectedApps";

export function readState(storage: Pick<Storage, "getItem"> | null): ConnectedAppsState {
  try {
    const raw = storage?.getItem(STATE_KEY);
    if (!raw) return EMPTY_STATE;
    const v = JSON.parse(raw) as Partial<ConnectedAppsState>;
    const list = (x: unknown) => (Array.isArray(x) ? x.filter((s): s is string => typeof s === "string") : []);
    return {
      known: Array.isArray(v.known) ? list(v.known) : null,
      off: list(v.off),
      auto: typeof v.auto === "boolean" ? v.auto : null,
    };
  } catch {
    return EMPTY_STATE;
  }
}

export function writeState(storage: Pick<Storage, "setItem"> | null, state: ConnectedAppsState): void {
  try {
    storage?.setItem(STATE_KEY, JSON.stringify(state));
  } catch {
    // Without storage Cua asks again next time; nothing is lost.
  }
}

export interface SyncPlan {
  /** The state to keep. */
  state: ConnectedAppsState;
  /** The new apps to set up now (each gets a notice with Undo). */
  setUp: string[];
}

/**
 * What to do after reading the apps on this computer. The first read only
 * records what is there. After that, an app that was not there before is set
 * up when the person said yes to that, unless they turned it off.
 */
export function planSync(rows: AgentSetupRow[], state: ConnectedAppsState): SyncPlan {
  const installed = rows.filter((r) => r.installed);
  const ids = installed.map((r) => r.agent);
  if (state.known === null) return { state: { ...state, known: ids }, setUp: [] };
  const known = new Set(state.known);
  const fresh = installed.filter((r) => !known.has(r.agent));
  const setUp = state.auto === true ? fresh.filter((r) => !r.configured && !state.off.includes(r.agent)).map((r) => r.agent) : [];
  return { state: { ...state, known: [...new Set([...state.known, ...ids])] }, setUp };
}

/** Turning an app on or off: off remembers it, on forgets that. */
export function withSwitch(state: ConnectedAppsState, agent: string, on: boolean): ConnectedAppsState {
  const off = on ? state.off.filter((a) => a !== agent) : [...new Set([...state.off, agent])];
  return { ...state, off };
}

/** The apps the person can switch: installed, or set up before the app went away. */
export const shownApps = (rows: AgentSetupRow[]) => rows.filter((r) => r.installed || r.configured);

/** Apps Cua knows that are not on this computer. */
export const missingApps = (rows: AgentSetupRow[]) => rows.filter((r) => !r.installed && !r.configured);

/** Where to get an app, for the ones we are sure of; the rest show no link. */
const GET_IT: Record<string, string> = {
  "claude-code": "https://claude.com/product/claude-code",
  codex: "https://developers.openai.com/codex",
  cursor: "https://cursor.com/download",
  vscode: "https://code.visualstudio.com/download",
  "gemini-cli": "https://github.com/google-gemini/gemini-cli",
};
export const getItUrl = (agent: string): string | null => GET_IT[agent] ?? null;

/** What to paste by hand for an app, when Cua can't edit its file. */
export const BY_HAND: Record<string, { file: string; snippet: string }> = {
  "claude-code": { file: "Terminal (writes ~/.claude.json)", snippet: "claude mcp add --scope user cua -- cua mcp" },
  codex: { file: "~/.codex/config.toml", snippet: '[mcp_servers.cua]\ncommand = "cua"\nargs = ["mcp"]' },
  hermes: { file: "~/.hermes/config.yaml", snippet: "mcp_servers:\n  cua:\n    command: cua\n    args: [mcp]" },
  openclaw: { file: "~/.openclaw/openclaw.json", snippet: '{\n  "mcp": {\n    "servers": {\n      "cua": { "command": "cua", "args": ["mcp"] }\n    }\n  }\n}' },
  cursor: { file: "~/.cursor/mcp.json", snippet: '{\n  "mcpServers": {\n    "cua": { "command": "cua", "args": ["mcp"] }\n  }\n}' },
};

/**
 * Why a setup failed, in a sentence a person can use. The setup's own words
 * ("Permission denied (os error 13)") stay out of sight.
 */
export function failureText(name: string, detail: string): string {
  if (/permission|read-only|readonly|EACCES|EPERM|os error 13|not writable/i.test(detail)) {
    return `Couldn't set up ${name}. Cua can't change this file because it is read-only or belongs to another user.`;
  }
  const said = detail.trim();
  return `Couldn't set up ${name}. ${!said || /^not set up$/i.test(said) ? "" : "Cua said: " + said.replace(/\.$/, "") + "."}`.trim();
}
