// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `agents.recentThreads` / `agents.threadHistory` (CUA-3319): the daemon's
// person-only tools with the web bridge's own argument mapping, the answer
// untouched, and an old daemon answering `unsupported`.
import { describe, expect, it, vi } from "vitest";
import { recentThreadsFromTool } from "../../cua-spaces-web/src/bridge/ops/agent-threads";
import { agentThreadsMethods } from "../src/bridge/agent-threads";
import type { BridgeContext } from "../src/bridge/context";

const ctx = {} as BridgeContext;
const LIST = { threads: [{ agent: "claude-code", id: "abc", title: "Kettle timer", cwd: "/Users/sam/code/kettle", last_active_ms: 5, resume: ["claude", "--resume", "abc"] }] };

describe("agents.recentThreads / agents.threadHistory", () => {
  it("lists with the tool's arguments and answers its JSON as-is", async () => {
    const tool = vi.fn(async () => LIST);
    const m = agentThreadsMethods(ctx, () => tool);
    const r = await m["agents.recentThreads"]!({ limit: 4 }, {});
    expect(tool).toHaveBeenCalledWith("agent_threads", { limit: 4 });
    expect(r).toBe(LIST);
    expect(recentThreadsFromTool(r)[0]!.title).toBe("Kettle timer");
  });

  it("passes the picked agents on, for the daemon to read only theirs", async () => {
    const tool = vi.fn(async () => LIST);
    const m = agentThreadsMethods(ctx, () => tool);
    await m["agents.recentThreads"]!({ limit: 4, agents: ["claude-code"] }, {});
    expect(tool).toHaveBeenCalledWith("agent_threads", { limit: 4, agents: ["claude-code"] });
  });

  it("reads one chat by its agent and id", async () => {
    const tool = vi.fn(async () => ({}));
    const m = agentThreadsMethods(ctx, () => tool);
    await m["agents.threadHistory"]!({ agent: "openai-codex", id: "x1" }, {});
    expect(tool).toHaveBeenCalledWith("agent_thread_read", { agent: "openai-codex", id: "x1" });
    await expect(m["agents.threadHistory"]!({ agent: "hermes" }, {})).rejects.toMatchObject({ code: "bad_args" });
  });

  it("a daemon without the tools is unsupported", async () => {
    const m = agentThreadsMethods(ctx, () => async () => Promise.reject(new Error("unknown tool agent_threads")));
    await expect(m["agents.recentThreads"]!({}, {})).rejects.toMatchObject({ code: "unsupported" });
  });
});
