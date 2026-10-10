// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings → Agents: the provider keys agents get (Anthropic, OpenAI and
 * other keys by variable name). The cua daemon keeps them in the Keychain
 * (`cua_spaces::agents::keys`) and gives each one only to the runs whose
 * harness reads it. Each operation is one of the daemon's app methods over
 * `SpaceService.CallSpaceTool` (`agent_keys.list`, `agent_keys.set`,
 * `agent_keys.remove`). They are not MCP tools: no agent can read or change
 * a key.
 *
 * Nothing here ever receives a key back: every answer is the list after the
 * change (provider, variable, last four characters, when it was added). A
 * value crosses the bridge once, in `agentKeys.set`'s arguments.
 *
 * | Operation | Tauri (`agents_tool`) | SwiftUI (`WebUIBridge+Pages.swift`) |
 * |---|---|---|
 * | `agentKeys.list` | `agent_keys.list` | `agentKeys.list` → `PersistentModel.daemonTool` |
 * | `agentKeys.set {provider, env?, value}` | `agent_keys.set` | `agentKeys.set` |
 * | `agentKeys.remove {env}` | `agent_keys.remove` | `agentKeys.remove` |
 *
 * The Electron shell answers the SwiftUI host's methods.
 */

import type { OpCoverage } from "../coverage";
import type { OpArgs, OpResult } from "../protocol";

/** `anthropic` and `openai` have their own rows; anything else is `other`. */
export type AgentKeyProvider = "anthropic" | "openai" | "other";

/** A stored key as the daemon describes it (`keys::KeyInfo`). Never its value. */
export interface AgentKeyInfo {
  provider: AgentKeyProvider;
  /** The variable a run gets it as. */
  env: string;
  /** Its last four characters; empty for a short key. */
  last4: string;
  /** When it was added (Unix ms). */
  addedMs: number;
}

/** A provider row (`keys::Provider`). */
export interface AgentKeyProviderRow {
  id: AgentKeyProvider;
  label: string;
  env: string;
}

/** `agent_keys.list` (`keys::KeysReport`). */
export interface AgentKeysReport {
  keys: AgentKeyInfo[];
  providers: AgentKeyProviderRow[];
  /** Keys can be saved on this machine. */
  available: boolean;
  /** Why not, when they can't. */
  unavailable: string | null;
}

declare module "../protocol" {
  interface HostOperations {
    /** The stored keys (no values) and whether this machine can keep them. */
    "agentKeys.list": { args: Record<string, never>; result: AgentKeysReport };
    /** Adds or replaces a key; `env` names an `other` key. Answers the list after. */
    "agentKeys.set": { args: { provider: AgentKeyProvider; env?: string | null; value: string }; result: AgentKeysReport };
    /** Removes the key named `env`. Answers the list after. */
    "agentKeys.remove": { args: { env: string }; result: AgentKeysReport };
  }
}

export const AGENT_KEYS_OPERATIONS = ["agentKeys.list", "agentKeys.set", "agentKeys.remove"] as const;
export type AgentKeysOp = (typeof AGENT_KEYS_OPERATIONS)[number];

/** The SwiftUI host routes each under its own name. */
export const AGENT_KEYS_WEBKIT_METHODS = AGENT_KEYS_OPERATIONS;

/** The daemon method each operation calls. */
export const AGENT_KEYS_METHODS = {
  "agentKeys.list": "agent_keys.list",
  "agentKeys.set": "agent_keys.set",
  "agentKeys.remove": "agent_keys.remove",
} as const satisfies { [K in AgentKeysOp]: string };

export const AGENT_KEYS_COVERAGE = {
  "agentKeys.list": { webkit: { methods: ["agentKeys.list"] }, tauri: ["agents_tool"] },
  "agentKeys.set": { webkit: { methods: ["agentKeys.set"] }, tauri: ["agents_tool"] },
  "agentKeys.remove": { webkit: { methods: ["agentKeys.remove"] }, tauri: ["agents_tool"] },
} as const satisfies Record<AgentKeysOp, OpCoverage>;

/** Arguments that take each operation down its usual path (the coverage test). */
export const AGENT_KEYS_ARGS: { [K in AgentKeysOp]: OpArgs<K> } = {
  "agentKeys.list": {},
  "agentKeys.set": { provider: "anthropic", value: "sk-ant-test-0000" },
  "agentKeys.remove": { env: "ANTHROPIC_API_KEY" },
};

/* ---- The daemon's answer ---------------------------------------------------------- */

const PROVIDERS: AgentKeyProviderRow[] = [
  { id: "anthropic", label: "Anthropic", env: "ANTHROPIC_API_KEY" },
  { id: "openai", label: "OpenAI", env: "OPENAI_API_KEY" },
];

const str = (v: unknown) => (typeof v === "string" ? v : "");
const providerOf = (v: unknown): AgentKeyProvider => (v === "anthropic" || v === "openai" ? v : "other");

/** The daemon's `KeysReport` (snake_case) as the page reads it. Drops anything
 * that looks like a value, so a host that answered one would not show it. */
export function agentKeysFromWire(raw: unknown): AgentKeysReport {
  const o = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  const keys = (Array.isArray(o.keys) ? o.keys : []).flatMap((k): AgentKeyInfo[] => {
    if (!k || typeof k !== "object") return [];
    const r = k as Record<string, unknown>;
    const env = str(r.env);
    if (!env) return [];
    const added = r.added_ms ?? r.addedMs;
    return [{ provider: providerOf(r.provider), env, last4: str(r.last4).slice(-4), addedMs: typeof added === "number" ? added : 0 }];
  });
  const providers = (Array.isArray(o.providers) ? o.providers : []).flatMap((p): AgentKeyProviderRow[] => {
    const r = (p ?? {}) as Record<string, unknown>;
    const id = providerOf(r.id);
    return id === "other" || !str(r.env) ? [] : [{ id, label: str(r.label) || id, env: str(r.env) }];
  });
  return {
    keys,
    providers: providers.length ? providers : PROVIDERS,
    available: o.available !== false,
    unavailable: typeof o.unavailable === "string" && o.unavailable ? o.unavailable : null,
  };
}

/** The arguments `agent_keys.set` takes. */
const setArgs = ({ provider, env, value }: OpArgs<"agentKeys.set">) =>
  provider === "other" ? { provider, env: (env ?? "").trim(), value } : { provider, value };

type Handlers = { [K in AgentKeysOp]: (args: OpArgs<K>) => Promise<OpResult<K>> };
type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
type Request = <T>(method: "agentKeys.list" | "agentKeys.set" | "agentKeys.remove", args?: Record<string, unknown>) => Promise<T>;

/** Tauri: the daemon's methods through `agents_tool` (the shell allows them). */
export function tauriAgentKeysOps(invoke: Invoke): Handlers {
  const tool = async (op: AgentKeysOp, args: Record<string, unknown> = {}) =>
    agentKeysFromWire(await invoke<unknown>("agents_tool", { tool: AGENT_KEYS_METHODS[op], args }));
  return {
    "agentKeys.list": () => tool("agentKeys.list"),
    "agentKeys.set": (args) => tool("agentKeys.set", setArgs(args)),
    "agentKeys.remove": ({ env }) => tool("agentKeys.remove", { env }),
  };
}

/** The SwiftUI host: each method answers the daemon's JSON as is. */
export function webkitAgentKeysOps(request: Request): Handlers {
  const call = async (op: AgentKeysOp, args: Record<string, unknown> = {}) => agentKeysFromWire(await request<unknown>(op, args));
  return {
    "agentKeys.list": () => call("agentKeys.list"),
    "agentKeys.set": (args) => call("agentKeys.set", setArgs(args)),
    "agentKeys.remove": ({ env }) => call("agentKeys.remove", { env }),
  };
}
