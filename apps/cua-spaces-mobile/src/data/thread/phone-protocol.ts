// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The wire for what the phone needs beyond the thread: tool rows, starting a task, changes and diffs. Shapes are the
 * ones agreed with the daemon (see docs/DESIGN.md; snake_case on the wire, camelCase
 * here). The sample backend produces these same JSON shapes and the live backend reads them, so a sample row and a
 * live row take the same path through the same mappers.
 */

type Json = Record<string, unknown>;

const obj = (v: unknown): Json | null => (v && typeof v === "object" && !Array.isArray(v) ? (v as Json) : null);
const str = (v: unknown, d = "") => (typeof v === "string" ? v : d);
const num = (v: unknown): number | null => (typeof v === "number" && Number.isFinite(v) ? v : null);
const arr = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);

/* ---- Tool rows (`thread_tool_rows`) ------------------------------------------------- */

export type ToolState = "running" | "done" | "failed";

export interface ToolCall {
  id: string;
  cat: string;
  line: string;
  detail: string;
  state: ToolState;
  atMs: number;
}
export interface ToolGroup {
  /** The thread seq of the message the calls follow. */
  afterSeq: number;
  agent: string;
  calls: ToolCall[];
}
export interface ToolRowsPage {
  groups: ToolGroup[];
  cursor: number;
  caughtUp: boolean;
}

export const toolRowsArgs = (a: { spaceId: string; cursor?: number | null; limit?: number | null; followMs?: number | null }): Json => ({
  space: a.spaceId,
  ...(a.cursor != null ? { cursor: a.cursor } : {}),
  ...(a.limit != null ? { limit: a.limit } : {}),
  ...(a.followMs ? { follow_ms: a.followMs } : {}),
});

const TOOL_STATES: readonly ToolState[] = ["running", "done", "failed"];

export function toolRowsFromTool(v: unknown): ToolRowsPage {
  const p = obj(v) ?? {};
  const groups = arr(p.groups).flatMap((g, gi): ToolGroup[] => {
    const o = obj(g);
    const after = num(o?.after_seq);
    if (!o || after === null) return [];
    const calls = arr(o.calls).flatMap((c, ci): ToolCall[] => {
      const k = obj(c);
      const line = str(k?.line);
      if (!k || !line) return [];
      const state = TOOL_STATES.includes(k.state as ToolState) ? (k.state as ToolState) : "done";
      return [{ id: str(k.id) || `${after}:${gi}:${ci}`, cat: str(k.cat, "other"), line, detail: str(k.detail), state, atMs: num(k.at_ms) ?? 0 }];
    });
    return calls.length ? [{ afterSeq: after, agent: str(o.agent), calls }] : [];
  });
  return { groups, cursor: num(p.cursor) ?? 0, caughtUp: p.caught_up !== false };
}

/** Folds a new page into what is known: calls are keyed by id, so a poll that repeats a call updates it (running to done). */
export function mergeToolRows(known: ToolGroup[], page: ToolGroup[]): ToolGroup[] {
  const by = new Map<string, ToolGroup>();
  const key = (g: ToolGroup) => `${g.afterSeq}\u0000${g.agent}`;
  for (const g of known) by.set(key(g), { ...g, calls: [...g.calls] });
  for (const g of page) {
    const have = by.get(key(g));
    if (!have) {
      by.set(key(g), { ...g, calls: [...g.calls] });
      continue;
    }
    for (const c of g.calls) {
      const i = have.calls.findIndex((x) => x.id === c.id);
      if (i >= 0) have.calls[i] = c;
      else have.calls.push(c);
    }
  }
  return [...by.values()].sort((a, b) => a.afterSeq - b.afterSeq);
}

/* ---- Start a task (`task_options`, `task_start`, `task_status`) ----------------------- */

/** What can start a task: machines (online, room, which agents are connected there) and agents. */
export interface TaskOptions {
  machines: { id: string; name: string; online: boolean; canCreate: boolean; why: string | null; os: string[]; readyAgents: string[] | null }[];
  agents: { harness: string; label: string; ready: boolean; why: string | null }[];
}

/**
 * One machine's `task_options`: `{machine, agents}` (what a daemon answers about itself), or `{machines, agents}` (a
 * list). The agents say which are connected there; `readyAgents` is that, kept on the machine's row.
 */
export function taskOptionsFromTool(v: unknown): TaskOptions {
  const p = obj(v) ?? {};
  const agents = arr(p.agents).flatMap((a) => {
    const o = obj(a);
    const harness = str(o?.harness);
    return o && harness ? [{ harness, label: str(o.label), ready: o.ready === true, why: str(o.why) || null }] : [];
  });
  const ready = agents.filter((a) => a.ready).map((a) => a.harness);
  const rows = obj(p.machine) ? [p.machine] : arr(p.machines);
  return {
    machines: rows.flatMap((m) => {
      const o = obj(m);
      const id = str(o?.id);
      return o && id
        ? [
            {
              id,
              name: str(o.name) || id,
              online: o.online === true,
              canCreate: o.can_create === true,
              why: str(o.why) || null,
              os: arr(o.os).filter((x): x is string => typeof x === "string"),
              readyAgents: obj(p.machine) ? ready : null,
            },
          ]
        : [];
    }),
    agents,
  };
}

/** The options of every machine asked, as one list: an agent is ready when it is connected on some machine. */
export function mergeTaskOptions(parts: TaskOptions[], names: Map<string, string> = new Map()): TaskOptions {
  const agents = new Map<string, TaskOptions["agents"][number]>();
  for (const part of parts) {
    for (const a of part.agents) {
      const have = agents.get(a.harness);
      if (!have || (a.ready && !have.ready)) agents.set(a.harness, { ...a, label: a.label || have?.label || "" });
    }
  }
  return {
    machines: parts.flatMap((p) => p.machines).map((m) => ({ ...m, name: names.get(m.id) ?? m.name })),
    agents: [...agents.values()],
  };
}

/** The machines that are known but offline: listed (disabled, "Offline") beside the ones that answered. */
export function offlineMachineOptions(known: { id: string; name: string; online: boolean }[], answered: Set<string>): TaskOptions["machines"] {
  return known
    .filter((m) => !m.online && !answered.has(m.id))
    .map((m) => ({ id: m.id, name: m.name, online: false, canCreate: false, why: null, os: [], readyAgents: null }));
}

export interface TaskResult {
  /** The Space the task is in (`relay:<id>`); empty while a new one is still being made. */
  space: string;
  /** This call made it (false: the same client id had already). */
  created: boolean;
  entrySeq: number | null;
  state: "ready" | "creating" | "failed";
  /** The key to follow a task that is being made (`task_status`). */
  pendingId: string | null;
  /** Why it failed, in the machine's words. */
  error: string | null;
  /** The agent's handle in the Space. */
  agent: string | null;
}

export function taskResultFromTool(v: unknown): TaskResult {
  const p = obj(v) ?? {};
  const state = p.state === "creating" ? "creating" : p.state === "failed" ? "failed" : "ready";
  const space = str(p.space);
  if (!space && state === "ready") throw new Error("The computer did not say where the task went.");
  return {
    space,
    created: p.created !== false,
    entrySeq: num(p.entry_seq),
    state,
    pendingId: str(p.pending_id) || null,
    error: str(p.error) || null,
    agent: str(p.agent) || null,
  };
}

export const taskStatusArgs = (clientId: string): Json => ({ client_id: clientId });

/* ---- Changes and diffs (`space_changes`, `space_file_diff`, `space_file_preview`) ---- */

export interface ChangeSet {
  repo: string;
  branch: string;
  base: string | null;
  headSha: string | null;
  /** The machine checked these against the push approval's card. */
  verified: boolean;
  files: {
    path: string;
    oldPath: string | null;
    status: string;
    additions: number | null;
    deletions: number | null;
    binary: boolean;
    size: number | null;
    mime: string | null;
  }[];
  truncated: boolean;
}

export const changesArgs = (a: { spaceId: string; repo?: string | null; base?: string | null }): Json => ({
  space: a.spaceId,
  ...(a.repo ? { repo: a.repo } : {}),
  ...(a.base ? { base: a.base } : {}),
});

export function changesFromTool(v: unknown): ChangeSet {
  const p = obj(v) ?? {};
  return {
    repo: str(p.repo),
    branch: str(p.branch),
    base: str(p.base) || null,
    headSha: str(p.head_sha) || null,
    verified: p.verified === true,
    truncated: p.truncated === true,
    files: arr(p.files).flatMap((f) => {
      const o = obj(f);
      const path = str(o?.path);
      return o && path
        ? [
            {
              path,
              oldPath: str(o.old_path) || null,
              status: str(o.status, "modified"),
              additions: num(o.additions),
              deletions: num(o.deletions),
              binary: o.binary === true,
              size: num(o.size),
              mime: str(o.mime) || null,
            },
          ]
        : [];
    }),
  };
}

export interface FileDiffData {
  path: string;
  patch: string;
  truncated: boolean;
  binary: boolean;
}

export const fileDiffArgs = (a: { spaceId: string; path: string; repo?: string | null; base?: string | null }): Json => ({
  space: a.spaceId,
  path: a.path,
  ...(a.repo ? { repo: a.repo } : {}),
  ...(a.base ? { base: a.base } : {}),
});

export function fileDiffFromTool(v: unknown): FileDiffData {
  const p = obj(v) ?? {};
  return { path: str(p.path), patch: str(p.patch), truncated: p.truncated === true, binary: p.binary === true };
}

export interface FilePreviewData {
  path: string;
  mime: string | null;
  size: number | null;
  encoding: "base64" | "utf8";
  content: string;
  truncated: boolean;
}

export const filePreviewArgs = (a: { spaceId: string; path: string; repo?: string | null; rev?: "head" | "base" | "working" }): Json => ({
  space: a.spaceId,
  path: a.path,
  ...(a.repo ? { repo: a.repo } : {}),
  ...(a.rev ? { rev: a.rev } : {}),
});

export function filePreviewFromTool(v: unknown): FilePreviewData {
  const p = obj(v) ?? {};
  return {
    path: str(p.path),
    mime: str(p.mime) || null,
    size: num(p.size),
    encoding: p.encoding === "base64" ? "base64" : "utf8",
    content: str(p.content),
    truncated: p.truncated === true,
  };
}
