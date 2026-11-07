// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * "Pick up where you left off" (CUA-3319): the recent Claude Code, Codex and
 * Hermes chats on this computer, read from each agent's own store, and one
 * chat's messages once the person picked it. Each operation is one of the
 * daemon's person-only tools (no MCP server lists or runs them, and the
 * daemon answers only this computer's own user):
 *
 * | Operation | Daemon tool | SwiftUI / Electron host | Tauri |
 * |---|---|---|---|
 * | `agents.recentThreads {limit?, agents?}` | `agent_threads` | the tool's JSON as-is | unsupported |
 * | `agents.threadHistory {agent, id, limit?}` | `agent_thread_read` | the tool's JSON as-is | unsupported |
 *
 * The hosts stay thin (`AGENT_THREADS_TOOLS` is the argument mapping, which
 * the Electron host imports); this file maps the answers for every host. A
 * daemon without the tools answers `unsupported`, and the Ready screen hides
 * the list. Only titles, folders and times are listed; nothing is kept.
 */

import { HostError, isUnsupported, UnsupportedOperationError } from "../adapter";
import type { OpCoverage } from "../coverage";

/** The agents whose chats are read (their harness ids). */
export type ThreadAgent = "claude-code" | "openai-codex" | "hermes";
export const THREAD_AGENTS: readonly ThreadAgent[] = ["claude-code", "openai-codex", "hermes"];

/** One chat as the list shows it (`cua_agent_threads::ThreadSummary`). */
export interface RecentThread {
  agent: ThreadAgent;
  /** The agent's own session id. */
  id: string;
  /** The chat's name, or its first prompt (at most 80 characters). */
  title: string;
  /** The folder it worked in. */
  cwd: string | null;
  lastActiveMs: number;
  /** The agent's own command that resumes it in a terminal (argv). */
  resume: string[];
}

/** One message of a chat (`cua_agent_threads::Message`). */
export interface PastMessage {
  key: string;
  role: "person" | "agent";
  text: string;
  tsMs: number | null;
  /** The text was cut (at 8 KiB). */
  cut: boolean;
}

export interface PastThread {
  thread: RecentThread;
  /** Oldest first. */
  messages: PastMessage[];
  /** Older messages left out. */
  earlierOmitted: number;
}

export interface AgentThreadsOps {
  /**
   * `agents`: only these agents' chats (harness or setup ids: `claude-code`, `codex`, `openai-codex`, `hermes`), read by
   * the daemon so every host lists the same; none: all three.
   */
  "agents.recentThreads": { args: { limit?: number; agents?: string[] }; result: RecentThread[] };
  "agents.threadHistory": { args: { agent: ThreadAgent; id: string; limit?: number }; result: PastThread };
}

export type AgentThreadsOpName = keyof AgentThreadsOps;

export const AGENT_THREADS_OPERATIONS = ["agents.recentThreads", "agents.threadHistory"] as const satisfies readonly AgentThreadsOpName[];
export const AGENT_THREADS_WEBKIT_METHODS = AGENT_THREADS_OPERATIONS;

const TAURI = "the coding agents' chats are read by the cua daemon's person-only tools, which the Tauri shell does not call";
export const AGENT_THREADS_COVERAGE = {
  "agents.recentThreads": { webkit: { methods: ["agents.recentThreads"] }, tauri: [], unsupported: { tauri: TAURI } },
  "agents.threadHistory": { webkit: { methods: ["agents.threadHistory"] }, tauri: [], unsupported: { tauri: TAURI } },
} as const satisfies Record<AgentThreadsOpName, OpCoverage>;

/** Each operation's daemon tool and its arguments (the native hosts use this mapping). */
export const AGENT_THREADS_TOOLS = {
  "agents.recentThreads": {
    tool: "agent_threads",
    args: (a: AgentThreadsOps["agents.recentThreads"]["args"]) => ({ ...(a.limit ? { limit: a.limit } : {}), ...(a.agents?.length ? { agents: [...a.agents] } : {}) }),
  },
  "agents.threadHistory": {
    tool: "agent_thread_read",
    args: (a: AgentThreadsOps["agents.threadHistory"]["args"]) => ({ agent: a.agent, id: a.id, ...(a.limit ? { limit: a.limit } : {}) }),
  },
} as const;

/** Arguments that take each down its usual path (coverage tests). */
export const AGENT_THREADS_ARGS = {
  "agents.recentThreads": { limit: 4 },
  "agents.threadHistory": { agent: "claude-code" as ThreadAgent, id: "0b1e5c2a-1111-4222-8333-944445555666" },
};

/* ---- The tools' JSON (snake_case) as the page reads it ------------------------------ */

const rec = (v: unknown): Record<string, unknown> => (v && typeof v === "object" ? (v as Record<string, unknown>) : {});
const str = (v: unknown): string => (typeof v === "string" ? v : "");
const num = (v: unknown): number => (typeof v === "number" && Number.isFinite(v) ? v : 0);

/** A harness or setup id (`codex`) as the chats' agent id; null for an agent whose chats aren't read. */
export function agentOf(v: unknown): ThreadAgent | null {
  const s = str(v);
  if (s === "codex") return "openai-codex";
  return (THREAD_AGENTS as readonly string[]).includes(s) ? (s as ThreadAgent) : null;
}

function threadFromTool(v: unknown): RecentThread | null {
  const t = rec(v);
  const agent = agentOf(t.agent);
  const id = str(t.id);
  if (!agent || !id) return null;
  return {
    agent,
    id,
    title: str(t.title) || "Untitled chat",
    cwd: typeof t.cwd === "string" && t.cwd ? t.cwd : null,
    lastActiveMs: num(t.last_active_ms),
    resume: Array.isArray(t.resume) ? t.resume.filter((x): x is string => typeof x === "string") : [],
  };
}

/** Only `agents`' chats (all when none are named): a daemon from before `agents` lists every agent's. */
export function onlyAgents(rows: RecentThread[], agents?: readonly string[]): RecentThread[] {
  if (!agents?.length) return rows;
  const want = new Set(agents.map(agentOf).filter((a): a is ThreadAgent => a !== null));
  return rows.filter((t) => want.has(t.agent));
}

/** `agent_threads`' answer; rows it can't read are left out. */
export function recentThreadsFromTool(v: unknown): RecentThread[] {
  const rows = rec(v).threads;
  return Array.isArray(rows) ? rows.map(threadFromTool).filter((t): t is RecentThread => t !== null) : [];
}

/** `agent_thread_read`'s answer; `asked` names the chat when the answer doesn't. */
export function pastThreadFromTool(v: unknown, asked?: { agent: ThreadAgent; id: string }): PastThread {
  const o = rec(v);
  const thread = threadFromTool(o.thread) ?? (asked ? { agent: asked.agent, id: asked.id, title: "Untitled chat", cwd: null, lastActiveMs: 0, resume: [] } : null);
  if (!thread) throw new HostError("The chat couldn't be read", "failed");
  const messages = (Array.isArray(o.messages) ? o.messages : [])
    .map((m): PastMessage | null => {
      const r = rec(m);
      const role = r.role === "person" || r.role === "agent" ? r.role : null;
      const text = str(r.text);
      if (!role || !text.trim()) return null;
      return { key: str(r.key), role, text, tsMs: typeof r.ts_ms === "number" ? r.ts_ms : null, cut: r.cut === true };
    })
    .filter((m): m is PastMessage => m !== null);
  return { thread, messages, earlierOmitted: num(o.earlier_omitted) };
}

/** A daemon from before these tools: its "unknown tool" is `unsupported`. */
export function isMissingAgentThreadsTool(e: unknown): boolean {
  const m = (e instanceof Error ? e.message : String(e)).toLowerCase();
  return ["agent_threads", "agent_thread_read"].some((t) => m.includes(`spaces tool ${t}`) || ((m.includes("unknown tool") || m.includes("no such tool")) && m.includes(t)));
}

type Request = <T>(method: AgentThreadsOpName, args?: Record<string, unknown>) => Promise<T>;

/** The SwiftUI and Electron hosts: the host method, then this file's mapping. */
export function webkitAgentThreadsOps(request: Request) {
  const call = async (op: AgentThreadsOpName, args: object) => {
    try {
      return await request<unknown>(op, { ...args });
    } catch (e) {
      if (!isUnsupported(e) && isMissingAgentThreadsTool(e)) throw new UnsupportedOperationError("webkit", op);
      throw e;
    }
  };
  return {
    "agents.recentThreads": async (a: AgentThreadsOps["agents.recentThreads"]["args"]) => onlyAgents(recentThreadsFromTool(await call("agents.recentThreads", a)), a.agents),
    "agents.threadHistory": async (a: AgentThreadsOps["agents.threadHistory"]["args"]) => pastThreadFromTool(await call("agents.threadHistory", a), a),
  };
}

/* ---- What the UI shows ------------------------------------------------------------- */

/** `/Users/sam/code/kettle` as `~/code/kettle`; a long path keeps its last two folders. */
export function folderLabel(cwd: string | null): string | null {
  if (!cwd) return null;
  const home = cwd.match(/^(\/Users\/[^/]+|\/home\/[^/]+|[A-Za-z]:\\Users\\[^\\]+)(?=$|[/\\])/);
  const short = home ? `~${cwd.slice(home[1]!.length)}` : cwd;
  const parts = short.split(/[/\\]/).filter(Boolean);
  return parts.length > 3 ? `…/${parts.slice(-2).join("/")}` : short || "~";
}

/** The earlier conversation as text for the agent, when it can't reload its own session (at most `max` bytes, newest kept). */
export function resumeContext(messages: readonly PastMessage[], max = 24 * 1024): string {
  const enc = new TextEncoder();
  const lines: string[] = [];
  let size = 0;
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i]!;
    const line = `${m.role === "person" ? "User" : "Agent"}: ${m.text.trim()}`;
    const n = enc.encode(line).length + 2;
    if (size + n > max) break;
    lines.unshift(line);
    size += n;
  }
  return lines.join("\n\n");
}
