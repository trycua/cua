// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// "Pick up where you left off" (CUA-3319): the daemon tools' JSON as the page reads it.

import { describe, expect, it } from "vitest";

import { AGENT_THREADS_TOOLS, folderLabel, isMissingAgentThreadsTool, onlyAgents, pastThreadFromTool, recentThreadsFromTool, resumeContext, webkitAgentThreadsOps } from "../ops/agent-threads";

const ROW = { agent: "claude-code", id: "abc", title: "Kettle timer", cwd: "/Users/sam/code/kettle", last_active_ms: 1_000, resume: ["claude", "--resume", "abc"] };

describe("agent threads", () => {
  it("maps the list, leaving out rows it can't read", () => {
    const out = recentThreadsFromTool({ threads: [ROW, { agent: "cursor", id: "x" }, { agent: "codex", id: "c1", title: "", cwd: null, last_active_ms: 2 }, "junk"] });
    expect(out).toEqual([
      { agent: "claude-code", id: "abc", title: "Kettle timer", cwd: "/Users/sam/code/kettle", lastActiveMs: 1_000, resume: ["claude", "--resume", "abc"] },
      { agent: "openai-codex", id: "c1", title: "Untitled chat", cwd: null, lastActiveMs: 2, resume: [] },
    ]);
    expect(recentThreadsFromTool(null)).toEqual([]);
  });

  it("maps one chat's messages", () => {
    const h = pastThreadFromTool({
      thread: ROW,
      messages: [
        { key: "m1", role: "person", text: "why?", ts_ms: 5 },
        { key: "m2", role: "agent", text: "time zones", ts_ms: null, cut: true },
        { key: "m3", role: "tool", text: "x" },
        { key: "m4", role: "agent", text: "  " },
      ],
      earlier_omitted: 3,
    });
    expect(h.messages).toEqual([
      { key: "m1", role: "person", text: "why?", tsMs: 5, cut: false },
      { key: "m2", role: "agent", text: "time zones", tsMs: null, cut: true },
    ]);
    expect(h.earlierOmitted).toBe(3);
    expect(() => pastThreadFromTool({})).toThrow();
  });

  it("shortens folders under the home folder", () => {
    expect(folderLabel("/Users/sam/code/kettle")).toBe("~/code/kettle");
    expect(folderLabel("/Users/sam")).toBe("~");
    expect(folderLabel("/home/sam/a/b/c/d")).toBe("…/c/d");
    expect(folderLabel("/srv/app")).toBe("/srv/app");
    expect(folderLabel(null)).toBeNull();
  });

  it("gives the agent the newest of the earlier conversation that fits", () => {
    const msgs = [
      { key: "1", role: "person" as const, text: "a".repeat(100), tsMs: null, cut: false },
      { key: "2", role: "agent" as const, text: "the answer", tsMs: null, cut: false },
      { key: "3", role: "person" as const, text: "thanks", tsMs: null, cut: false },
    ];
    expect(resumeContext(msgs)).toBe(`User: ${"a".repeat(100)}\n\nAgent: the answer\n\nUser: thanks`);
    expect(resumeContext(msgs, 40)).toBe("Agent: the answer\n\nUser: thanks");
  });

  it("asks the daemon for the picked agents only, and keeps only theirs from an older daemon", async () => {
    const args = AGENT_THREADS_TOOLS["agents.recentThreads"].args;
    expect(args({ limit: 4, agents: ["claude-code"] })).toEqual({ limit: 4, agents: ["claude-code"] });
    expect(args({ limit: 4, agents: [] })).toEqual({ limit: 4 });
    const rows = recentThreadsFromTool({
      threads: [
        { agent: "claude-code", id: "a", title: "A", last_active_ms: 3 },
        { agent: "openai-codex", id: "b", title: "B", last_active_ms: 2 },
        { agent: "hermes", id: "c", title: "C", last_active_ms: 1 },
      ],
    });
    expect(onlyAgents(rows, ["claude-code"]).map((t) => t.id)).toEqual(["a"]);
    // The setup's id for Codex is `codex`.
    expect(onlyAgents(rows, ["codex"]).map((t) => t.id)).toEqual(["b"]);
    expect(onlyAgents(rows, ["claude-code", "codex"]).map((t) => t.id)).toEqual(["a", "b"]);
    expect(onlyAgents(rows, ["gemini-cli"])).toEqual([]);
    expect(onlyAgents(rows)).toHaveLength(3);
    const seen: unknown[] = [];
    const ops = webkitAgentThreadsOps(<T,>(_m: string, a?: Record<string, unknown>) => (seen.push(a), Promise.resolve({ threads: [{ agent: "openai-codex", id: "b", title: "B" }] } as T)));
    expect(await ops["agents.recentThreads"]({ agents: ["claude-code"] })).toEqual([]);
    expect(seen).toEqual([{ agents: ["claude-code"] }]);
  });

  it("a daemon without the tools is unsupported", async () => {
    expect(isMissingAgentThreadsTool(new Error("unknown tool agent_threads"))).toBe(true);
    expect(isMissingAgentThreadsTool(new Error("NotFound: Spaces tool agent_thread_read"))).toBe(true);
    expect(isMissingAgentThreadsTool(new Error("no claude-code chat abc on this computer"))).toBe(false);
    const ops = webkitAgentThreadsOps(async () => Promise.reject(new Error("unknown tool agent_threads")));
    await expect(ops["agents.recentThreads"]({})).rejects.toMatchObject({ code: "unsupported" });
  });
});
