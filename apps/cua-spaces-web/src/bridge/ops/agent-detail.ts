// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * One persistent agent's Memory, Routines and Access to this machine: the
 * commands the app core's Agents page asks for (`AgentsRequest` in
 * `cua-spaces-app-core/src/persistent.rs`), one operation each. Every one is
 * a daemon Spaces tool the SwiftUI app's `PersistentModel.send` already
 * calls; the page reduces and draws with the core (`agent-detail.ts`).
 *
 * | Operation | Daemon tools | SwiftUI / Electron host |
 * |---|---|---|
 * | `agents.detail {name}` | `volume_ls` (the home, recursively), `routine_list`, `computer_access_list` (audit 20) | `PersistentModel.detail` |
 * | `agents.readFile {path}` | `volume_read` + `volume_history` | `PersistentModel.readFile` |
 * | `agents.restoreFile {path, version}` | `volume_restore` | `PersistentModel.perform(.restore)` |
 * | `agents.addRoutine {agent, title, prompt, everyMinutes?, dailyAt?, weeklyOn?}` | `routine_add` | `.addRoutine` |
 * | `agents.setRoutine {id, enabled}` | `routine_set_enabled` | `.setRoutine` |
 * | `agents.removeRoutine {id}` | `routine_remove` | `.removeRoutine` |
 * | `agents.allowComputer {agent, machine}` | `computer_access_grant` (the daemon asks for presence) | `.allow` |
 * | `agents.revokeComputer {agent, machine}` | `computer_access_revoke` | `.revoke` |
 *
 * The SwiftUI and Electron hosts answer `agents.detail` and `agents.readFile`
 * as the core's input records (camelCase); Tauri maps the tools' JSON here.
 */

import type { OpCoverage } from "../coverage";
import type { AgentDetailInput, AgentOpenFile } from "../contracts/agents";
import type { OpArgs, OpResult } from "../protocol";

type Ok = null;

declare module "../protocol" {
  interface HostOperations {
    /** The agent's home files, its routines, every computer grant and the recent audit. */
    "agents.detail": { args: { name: string }; result: AgentDetailInput };
    /** A home file's text and versions. */
    "agents.readFile": { args: { path: string }; result: AgentOpenFile };
    "agents.restoreFile": { args: { path: string; version: string }; result: Ok };
    /** Exactly one of `everyMinutes`, `dailyAt` (`HH:MM`) and `weeklyOn` (`mon HH:MM`). */
    "agents.addRoutine": {
      args: { agent: string; title: string; prompt: string; everyMinutes: number | null; dailyAt: string | null; weeklyOn: string | null };
      result: Ok;
    };
    "agents.setRoutine": { args: { id: string; enabled: boolean }; result: Ok };
    "agents.removeRoutine": { args: { id: string }; result: Ok };
    /** Lets the agent use `machine` (the daemon asks for presence first). */
    "agents.allowComputer": { args: { agent: string; machine: string }; result: Ok };
    "agents.revokeComputer": { args: { agent: string; machine: string }; result: Ok };
  }
}

export const AGENT_DETAIL_OPERATIONS = [
  "agents.detail",
  "agents.readFile",
  "agents.restoreFile",
  "agents.addRoutine",
  "agents.setRoutine",
  "agents.removeRoutine",
  "agents.allowComputer",
  "agents.revokeComputer",
] as const;
export type AgentDetailOp = (typeof AGENT_DETAIL_OPERATIONS)[number];

/** The SwiftUI host and the Electron shell route each under its own name. */
export const AGENT_DETAIL_WEBKIT_METHODS = AGENT_DETAIL_OPERATIONS;

export const AGENT_DETAIL_COVERAGE = {
  "agents.detail": { webkit: { methods: ["agents.detail"] }, tauri: ["agents_tool"] },
  "agents.readFile": { webkit: { methods: ["agents.readFile"] }, tauri: ["agents_tool"] },
  "agents.restoreFile": { webkit: { methods: ["agents.restoreFile"] }, tauri: ["agents_tool"] },
  "agents.addRoutine": { webkit: { methods: ["agents.addRoutine"] }, tauri: ["agents_tool"] },
  "agents.setRoutine": { webkit: { methods: ["agents.setRoutine"] }, tauri: ["agents_tool"] },
  "agents.removeRoutine": { webkit: { methods: ["agents.removeRoutine"] }, tauri: ["agents_tool"] },
  "agents.allowComputer": { webkit: { methods: ["agents.allowComputer"] }, tauri: ["agents_tool"] },
  "agents.revokeComputer": { webkit: { methods: ["agents.revokeComputer"] }, tauri: ["agents_tool"] },
} as const satisfies Record<AgentDetailOp, OpCoverage>;

/** Arguments that take each operation down its usual path (the coverage test). */
export const AGENT_DETAIL_ARGS: { [K in AgentDetailOp]: OpArgs<K> } = {
  "agents.detail": { name: "ada" },
  "agents.readFile": { path: "agents/ada/notes.md" },
  "agents.restoreFile": { path: "agents/ada/notes.md", version: "v1" },
  "agents.addRoutine": { agent: "ada", title: "Inbox", prompt: "Sort my inbox", everyMinutes: null, dailyAt: "08:00", weeklyOn: null },
  "agents.setRoutine": { id: "r1", enabled: false },
  "agents.removeRoutine": { id: "r1" },
  "agents.allowComputer": { agent: "ada", machine: "relay:this-mac" },
  "agents.revokeComputer": { agent: "ada", machine: "relay:this-mac" },
};

/* ---- The tools' JSON as the core reads it (PersistentModel.loadDetail) ------------- */

type Json = Record<string, unknown>;
const rows = (v: unknown, key: string): Json[] => {
  const a = v && typeof v === "object" ? (v as Json)[key] : undefined;
  return Array.isArray(a) ? a.filter((x): x is Json => !!x && typeof x === "object") : [];
};
const str = (v: unknown) => (typeof v === "string" ? v : "");
const num = (v: unknown) => (typeof v === "number" ? v : 0);

/** At most 50 folders and 500 files, as the native hosts read a home. */
const MAX_FOLDERS = 50;
const MAX_FILES = 500;

type Tool = (tool: string, args: Json) => Promise<unknown>;

/** `agents.detail` from the daemon's tools (what the native hosts do in Swift and in the Electron main process). */
export async function agentDetailFromTools(tool: Tool, name: string, thisMachine: string | null): Promise<AgentDetailInput> {
  const home: AgentDetailInput["home"] = [];
  const queue = [`agents/${name}/`];
  for (let folders = 0; queue.length && folders < MAX_FOLDERS && home.length < MAX_FILES; folders++) {
    for (const e of rows(await tool("volume_ls", { path: queue.shift()! }), "entries")) {
      if (e.folder === true) queue.push(str(e.path));
      else home.push({ path: str(e.path), name: str(e.name), size: num(e.size) });
    }
  }
  const routines = rows(await tool("routine_list", { agent: name }), "routines").map((r) => ({
    id: str(r.id),
    title: str(r.title),
    label: str(r.label),
    enabled: typeof r.isEnabled === "boolean" ? r.isEnabled : true,
  }));
  const access = await tool("computer_access_list", { audit: 20 });
  return {
    home,
    routines,
    grants: rows(access, "grants").map((g) => ({ agent: str(g.agent), machine: str(g.machine), revoked: g.revoked === true })),
    audit: rows(access, "audit").map((a) => ({ tsMs: num(a.ts_ms), action: str(a.action), principal: str(a.principal), path: str(a.path), detail: str(a.detail) })),
    thisMachine,
  };
}

/** `agents.readFile` from `volume_read` and `volume_history`. */
export async function agentFileFromTools(tool: Tool, path: string): Promise<AgentOpenFile> {
  const [file, history] = await Promise.all([tool("volume_read", { path }), tool("volume_history", { path })]);
  const f = (file ?? {}) as Json;
  const binary = f.encoding === "base64";
  return {
    path,
    text: binary ? "" : str(f.content),
    binary,
    versions: rows(history, "versions").map((v) => ({ version: str(v.version), modifiedMs: num(v.modified_ms), deleted: v.deleted === true, latest: v.latest === true })),
  };
}

/** `routine_add`'s arguments: only the schedule field that is set. */
export function routineAddArgs(a: OpArgs<"agents.addRoutine">): Json {
  const out: Json = { agent: a.agent, title: a.title, prompt: a.prompt };
  if (a.everyMinutes !== null && a.everyMinutes !== undefined) out.every_minutes = a.everyMinutes;
  if (a.dailyAt) out.daily_at = a.dailyAt;
  if (a.weeklyOn) out.weekly_on = a.weeklyOn;
  return out;
}

type Handlers = { [K in AgentDetailOp]: (args: OpArgs<K>) => Promise<OpResult<K>> };
type Invoke = <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
type Request = <T>(method: AgentDetailOp, args?: Record<string, unknown>) => Promise<T>;

/** Each mutation: one tool, then nothing to answer. */
function mutations(run: (tool: string, args: Json) => Promise<unknown>): Omit<Handlers, "agents.detail" | "agents.readFile"> {
  const done = (tool: string, args: Json) => run(tool, args).then(() => null);
  return {
    "agents.restoreFile": ({ path, version }) => done("volume_restore", { path, version }),
    "agents.addRoutine": (args) => done("routine_add", routineAddArgs(args)),
    "agents.setRoutine": ({ id, enabled }) => done("routine_set_enabled", { id, enabled }),
    "agents.removeRoutine": ({ id }) => done("routine_remove", { id }),
    "agents.allowComputer": ({ agent, machine }) => done("computer_access_grant", { agent, machine }),
    "agents.revokeComputer": ({ agent, machine }) => done("computer_access_revoke", { agent, machine }),
  };
}

/** Tauri: the tools through `agents_tool` (all on its allow-list). The Tauri
 * shell does not know this machine's relay Space, so it offers no "Allow". */
export function tauriAgentDetailOps(invoke: Invoke): Handlers {
  const tool: Tool = (name, args) => invoke("agents_tool", { tool: name, args });
  return {
    "agents.detail": ({ name }) => agentDetailFromTools(tool, name, null),
    "agents.readFile": ({ path }) => agentFileFromTools(tool, path),
    ...mutations(tool),
  };
}

/** The SwiftUI host and the Electron shell: each method keeps its operation's name and arguments. */
export function webkitAgentDetailOps(request: Request): Handlers {
  const none = (op: AgentDetailOp) => async (args: object) => {
    await request(op, { ...args });
    return null;
  };
  return {
    "agents.detail": async ({ name }) => {
      const d = (await request<Partial<AgentDetailInput> | null>("agents.detail", { name })) ?? {};
      return { home: d.home ?? [], routines: d.routines ?? [], grants: d.grants ?? [], audit: d.audit ?? [], thisMachine: d.thisMachine ?? null };
    },
    "agents.readFile": ({ path }) => request("agents.readFile", { path }),
    "agents.restoreFile": none("agents.restoreFile"),
    "agents.addRoutine": none("agents.addRoutine"),
    "agents.setRoutine": none("agents.setRoutine"),
    "agents.removeRoutine": none("agents.removeRoutine"),
    "agents.allowComputer": none("agents.allowComputer"),
    "agents.revokeComputer": none("agents.revokeComputer"),
  };
}
