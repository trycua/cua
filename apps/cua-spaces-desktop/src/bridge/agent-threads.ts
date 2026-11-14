// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// "Pick up where you left off" (CUA-3319): `agents.recentThreads` and
// `agents.threadHistory`, the daemon's person-only tools `agent_threads` and
// `agent_thread_read`, as the SwiftUI host runs them. The arguments map with
// the web bridge's own `AGENT_THREADS_TOOLS` and the answer is the tool's
// JSON untouched (the page maps it). A daemon without the tools answers
// `unsupported`, and the Ready screen hides the list.
import {
  AGENT_THREADS_OPERATIONS,
  AGENT_THREADS_TOOLS,
  isMissingAgentThreadsTool,
  type AgentThreadsOpName,
} from "../../../cua-spaces-web/src/bridge/ops/agent-threads";
import { string } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { daemonTool } from "./host-parts";

type Tool = (name: string, args?: Record<string, unknown>) => Promise<unknown>;

/** `tool`: the daemon's tools (tests pass their own). */
export function agentThreadsMethods(ctx: BridgeContext, tool: () => Tool = () => daemonTool(ctx, "Your agents' chats")): Handlers {
  const run = (op: AgentThreadsOpName) => async (args: Record<string, unknown>) => {
    if (op === "agents.threadHistory") {
      string(args, "agent");
      string(args, "id");
    }
    const call = AGENT_THREADS_TOOLS[op] as { tool: string; args: (a: never) => Record<string, unknown> };
    try {
      return await tool()(call.tool, call.args(args as never));
    } catch (e) {
      if (isMissingAgentThreadsTool(e)) throw Failure.unsupported("This cua daemon can't list your agents' chats; update Cua");
      throw e;
    }
  };
  return Object.fromEntries(AGENT_THREADS_OPERATIONS.map((op) => [op, run(op)])) as Handlers;
}
