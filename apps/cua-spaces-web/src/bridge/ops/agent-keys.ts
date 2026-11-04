// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings → Connections: the provider keys agents get (one per provider of
 * the registry, and other keys by variable name). The cua daemon keeps them in the Keychain
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
 * | `agentKeys.remove {env, slot?, keepOthers?}` | `agent_keys.remove` | `agentKeys.remove` |
 * | `agentKeys.use {app, env?, slot?}` | `agent_keys.use` | `agentKeys.use` |
 *
 * The Electron shell answers the SwiftUI host's methods.
 */

import type { OpCoverage } from "../coverage";
import type { OpArgs, OpResult } from "../protocol";

/** A provider id of the registry (`anthropic`, `deepseek`, `custom`, ...), or `other` for a key saved by variable name. */
export type AgentKeyProvider = string;

/** A stored key as the daemon describes it (`keys::KeyInfo`). Never its value. */
export interface AgentKeyInfo {
  provider: AgentKeyProvider;
  /** The variable a run gets it as. */
  env: string;
  /** Which of several keys saved for the same variable this is; absent for the first one. */
  slot?: string;
  /** Its last four characters; empty for a short key. */
  last4: string;
  /** When it was added (Unix ms). */
  addedMs: number;
  /** A sign-in past its expiry: no run gets it until it is reconnected. */
  expired?: boolean;
  /** A copied sign-in's name ("Signed in as ada@example.com (Claude Max)"); never a value. */
  label?: string;
  /** The plan or region chosen for it (Z.AI), when the provider has them. */
  region?: string;
  /** Non-secret values saved with it: a custom endpoint's `base_url`, `model` and `wire`, or a `model`. */
  fields?: Record<string, string>;
}

/** The non-secret choices a connection carries (`agentKeys.set`, `agentKeys.fromShell`). */
export interface KeyChoices {
  /** A plan or region the provider lists. */
  region?: string | null;
  /** `model`, and for a custom endpoint `base_url` and `wire`. */
  fields?: Record<string, string>;
}

/** The credential an app was told to use (`keys::UseChoice`): names only. */
export interface AgentKeyUse {
  provider: AgentKeyProvider;
  env: string;
  slot?: string;
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
  /** Which credential each app that takes any provider uses, by app (an app not listed uses the first that works). */
  uses?: Record<string, AgentKeyUse>;
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
    /**
     * The list, after asking each provider whether the saved API keys still work (a cheap read-only
     * request, answers reused for a few minutes, never logged). A key the provider refuses comes back
     * `expired`: it needs a reconnect. A sign-in token has no check that is known to accept it, so it
     * is listed as it is.
     */
    "agentKeys.check": { args: Record<string, never>; result: AgentKeysReport };
    /**
     * Adds or replaces a key; `env` names an `other` key. With `add` it is saved as another key
     * beside the first one for the provider (named `label`); it replaces nothing. Answers the list after.
     */
    "agentKeys.set": { args: { provider: AgentKeyProvider; env?: string | null; value: string; add?: boolean; label?: string } & KeyChoices; result: AgentKeysReport };
    /**
     * Removes the key named `env` and every key saved beside it, or with `slot` only that one, or with
     * `keepOthers` only the first (the oldest of the others takes its place). Answers the list after.
     */
    "agentKeys.remove": { args: { env: string; slot?: string; keepOthers?: boolean }; result: AgentKeysReport };
    /**
     * Tells an app that takes any provider (Hermes, OpenCode, Goose, Pi, OpenClaw) which saved credential to
     * use from its next run; without `env`, the first one that works again. Signs nothing out and deletes
     * nothing. Answers the list after.
     */
    "agentKeys.use": { args: { app: string; env?: string; slot?: string }; result: AgentKeysReport };
    /** Which provider keys your login shell has set (names only; never a value). */
    "agentKeys.shell": { args: Record<string, never>; result: { names: string[] } };
    /** Saves the key `env` from your login shell (the value goes straight to the store). Answers the list after. */
    "agentKeys.fromShell": { args: { env: string } & KeyChoices; result: AgentKeysReport };
  }
}

export const AGENT_KEYS_OPERATIONS = ["agentKeys.list", "agentKeys.check", "agentKeys.set", "agentKeys.remove", "agentKeys.use", "agentKeys.shell", "agentKeys.fromShell"] as const;
export type AgentKeysOp = (typeof AGENT_KEYS_OPERATIONS)[number];

/** The SwiftUI host routes each under its own name. */
export const AGENT_KEYS_WEBKIT_METHODS = AGENT_KEYS_OPERATIONS;

/** The daemon method each operation calls. */
export const AGENT_KEYS_METHODS = {
  "agentKeys.list": "agent_keys.list",
  "agentKeys.check": "agent_keys.check",
  "agentKeys.set": "agent_keys.set",
  "agentKeys.remove": "agent_keys.remove",
  "agentKeys.use": "agent_keys.use",
  "agentKeys.shell": "agent_keys.shell",
  "agentKeys.fromShell": "agent_keys.from_shell",
} as const satisfies { [K in AgentKeysOp]: string };

export const AGENT_KEYS_COVERAGE = {
  "agentKeys.list": { webkit: { methods: ["agentKeys.list"] }, tauri: ["agents_tool"] },
  "agentKeys.check": { webkit: { methods: ["agentKeys.check"] }, tauri: ["agents_tool"] },
  "agentKeys.set": { webkit: { methods: ["agentKeys.set"] }, tauri: ["agents_tool"] },
  "agentKeys.remove": { webkit: { methods: ["agentKeys.remove"] }, tauri: ["agents_tool"] },
  "agentKeys.use": { webkit: { methods: ["agentKeys.use"] }, tauri: ["agents_tool"] },
  "agentKeys.shell": { webkit: { methods: ["agentKeys.shell"] }, tauri: ["agents_tool"] },
  "agentKeys.fromShell": { webkit: { methods: ["agentKeys.fromShell"] }, tauri: ["agents_tool"] },
} as const satisfies Record<AgentKeysOp, OpCoverage>;

/** Arguments that take each operation down its usual path (the coverage test). */
export const AGENT_KEYS_ARGS: { [K in AgentKeysOp]: OpArgs<K> } = {
  "agentKeys.list": {},
  "agentKeys.check": {},
  "agentKeys.set": { provider: "anthropic", value: "sk-ant-test-0000" },
  "agentKeys.remove": { env: "ANTHROPIC_API_KEY" },
  "agentKeys.use": { app: "hermes", env: "OPENAI_API_KEY" },
  "agentKeys.shell": {},
  "agentKeys.fromShell": { env: "ANTHROPIC_API_KEY" },
};

/* ---- The daemon's answer ---------------------------------------------------------- */

const PROVIDERS: AgentKeyProviderRow[] = [
  { id: "anthropic", label: "Anthropic", env: "ANTHROPIC_API_KEY" },
  { id: "openai", label: "OpenAI", env: "OPENAI_API_KEY" },
];

const str = (v: unknown) => (typeof v === "string" ? v : "");
/** A sign-in's name: one short line of text, else none. */
const label = (v: unknown): string => (typeof v === "string" && v.length <= 160 && !/[\u0000-\u001f]/.test(v) ? v.trim() : "");
const providerOf = (v: unknown): AgentKeyProvider => (typeof v === "string" && /^[a-z0-9-]{1,40}$/.test(v) ? v : "other");
/** A short plain line, else none (a region id, a field value). */
const line = (v: unknown): string => (typeof v === "string" && v.length <= 512 && !/[\u0000-\u001f]/.test(v) ? v.trim() : "");
const fieldsOf = (v: unknown): Record<string, string> => {
  if (!v || typeof v !== "object" || Array.isArray(v)) return {};
  return Object.fromEntries(Object.entries(v as Record<string, unknown>).flatMap(([k, x]) => (/^[a-z_]{1,24}$/.test(k) && line(x) ? [[k, line(x)]] : [])));
};

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
    const slot = typeof r.slot === "string" && /^[A-Za-z0-9_-]{1,40}$/.test(r.slot) ? r.slot : "";
    return [{ provider: providerOf(r.provider), env, ...(slot ? { slot } : {}), last4: str(r.last4).slice(-4), addedMs: typeof added === "number" ? added : 0, ...(r.expired === true ? { expired: true } : {}), ...(label(r.label) ? { label: label(r.label) } : {}), ...(line(r.region) ? { region: line(r.region) } : {}), ...(Object.keys(fieldsOf(r.fields)).length ? { fields: fieldsOf(r.fields) } : {}) }];
  });
  const providers = (Array.isArray(o.providers) ? o.providers : []).flatMap((p): AgentKeyProviderRow[] => {
    const r = (p ?? {}) as Record<string, unknown>;
    const id = providerOf(r.id);
    return id === "other" || !str(r.env) ? [] : [{ id, label: str(r.label) || id, env: str(r.env) }];
  });
  const uses: Record<string, AgentKeyUse> = {};
  for (const [app, u] of Object.entries(o.uses && typeof o.uses === "object" && !Array.isArray(o.uses) ? (o.uses as Record<string, unknown>) : {})) {
    const r = (u ?? {}) as Record<string, unknown>;
    if (!/^[a-z0-9-]{1,40}$/.test(app) || !str(r.env)) continue;
    const slot = typeof r.slot === "string" && /^[A-Za-z0-9_-]{1,40}$/.test(r.slot) ? r.slot : "";
    uses[app] = { provider: providerOf(r.provider), env: str(r.env), ...(slot ? { slot } : {}) };
  }
  return {
    keys,
    ...(Object.keys(uses).length ? { uses } : {}),
    providers: providers.length ? providers : PROVIDERS,
    available: o.available !== false,
    unavailable: typeof o.unavailable === "string" && o.unavailable ? o.unavailable : null,
  };
}

/** The arguments `agent_keys.set` takes. */
const choicesOf = ({ region, fields }: KeyChoices) => ({
  ...(region ? { region } : {}),
  ...(fields && Object.keys(fields).length ? { fields } : {}),
});
const setArgs = ({ provider, env, value, region, fields, add, label }: OpArgs<"agentKeys.set">) => {
  const more = { ...(add ? { add: true } : {}), ...(label?.trim() ? { label: label.trim() } : {}) };
  return provider === "other" ? { provider, env: (env ?? "").trim(), value, ...choicesOf({ region, fields }), ...more } : { provider, value, ...choicesOf({ region, fields }), ...more };
};
/** The arguments `agent_keys.remove` takes (the daemon's `keep_others`). */
const removeArgs = ({ env, slot, keepOthers }: OpArgs<"agentKeys.remove">) => ({ env, ...(slot ? { slot } : {}), ...(keepOthers ? { keep_others: true } : {}) });

type Handlers = { [K in AgentKeysOp]: (args: OpArgs<K>) => Promise<OpResult<K>> };
type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
type Request = <T>(method: AgentKeysOp, args?: Record<string, unknown>) => Promise<T>;

/** `agent_keys.shell`'s answer: names only. */
const shellNamesFromWire = (raw: unknown): { names: string[] } => {
  const o = (raw && typeof raw === "object" ? raw : {}) as Record<string, unknown>;
  return { names: (Array.isArray(o.names) ? o.names : []).filter((n): n is string => typeof n === "string") };
};

/** Tauri: the daemon's methods through `agents_tool` (the shell allows them). */
export function tauriAgentKeysOps(invoke: Invoke): Handlers {
  const tool = async (op: AgentKeysOp, args: Record<string, unknown> = {}) =>
    agentKeysFromWire(await invoke<unknown>("agents_tool", { tool: AGENT_KEYS_METHODS[op], args }));
  return {
    "agentKeys.list": () => tool("agentKeys.list"),
    "agentKeys.check": () => tool("agentKeys.check"),
    "agentKeys.set": (args) => tool("agentKeys.set", setArgs(args)),
    "agentKeys.remove": (args) => tool("agentKeys.remove", removeArgs(args)),
    "agentKeys.use": ({ app, env, slot }) => tool("agentKeys.use", { app, ...(env ? { env } : {}), ...(slot ? { slot } : {}) }),
    "agentKeys.shell": async () => shellNamesFromWire(await invoke<unknown>("agents_tool", { tool: AGENT_KEYS_METHODS["agentKeys.shell"], args: {} })),
    "agentKeys.fromShell": ({ env, region, fields }) => tool("agentKeys.fromShell", { env, ...choicesOf({ region, fields }) }),
  };
}

/** The SwiftUI host: each method answers the daemon's JSON as is. */
export function webkitAgentKeysOps(request: Request): Handlers {
  const call = async (op: AgentKeysOp, args: Record<string, unknown> = {}) => agentKeysFromWire(await request<unknown>(op, args));
  return {
    "agentKeys.list": () => call("agentKeys.list"),
    "agentKeys.check": () => call("agentKeys.check"),
    "agentKeys.set": (args) => call("agentKeys.set", setArgs(args)),
    "agentKeys.remove": (args) => call("agentKeys.remove", removeArgs(args)),
    "agentKeys.use": ({ app, env, slot }) => call("agentKeys.use", { app, ...(env ? { env } : {}), ...(slot ? { slot } : {}) }),
    "agentKeys.shell": async () => shellNamesFromWire(await request<unknown>("agentKeys.shell", {})),
    "agentKeys.fromShell": ({ env, region, fields }) => call("agentKeys.fromShell", { env, ...choicesOf({ region, fields }) }),
  };
}
