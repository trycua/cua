// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Space thread (`thread.read`, `thread.post`, `thread.members`,
// `thread.setRights`, CUA-2155/2156; `thread.agentAdd`, `thread.agentRemove`,
// `thread.joinCodeCreate`, `thread.joinCodeList`, `thread.joinCodeCancel`,
// `thread.settings`, CUA-2263): the daemon's tools named in the web bridge's
// `THREAD_TOOLS`, as the SwiftUI host runs them. The arguments map with the web bridge's own
// `THREAD_TOOLS` and the answer is the tool's JSON untouched (the page maps
// it), so the mapping lives once. A daemon (or SDK) without the thread
// answers `unsupported`, and the page keeps its classic layout.
import { isMissingThreadTool, THREAD_OPERATIONS, THREAD_TOOLS, type ThreadOpName } from "../../../cua-spaces-web/src/bridge/ops/thread";
import { string } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { daemonTool } from "./host-parts";

type Tool = (name: string, args?: Record<string, unknown>) => Promise<unknown>;

/** `tool`: the daemon's tools (tests pass their own). */
export function threadMethods(ctx: BridgeContext, tool: () => Tool = () => daemonTool(ctx, "The Space thread")): Handlers {
  const run = (op: ThreadOpName) => async (args: Record<string, unknown>) => {
    string(args, "spaceId");
    if (op === "thread.post") string(args, "text");
    if (op === "thread.setRights" || op === "thread.agentAdd" || op === "thread.agentRemove") string(args, "name");
    if (op === "thread.agentAdd") string(args, "harness");
    if (op === "thread.joinCodeCancel") string(args, "id");
    if (op === "thread.joinCodeCreate" && args.canTask !== undefined && args.canTask !== "owner" && args.canTask !== "members") {
      throw Failure.badArgs("canTask: owner or members");
    }
    const call = THREAD_TOOLS[op] as { tool: string; args: (a: never) => Record<string, unknown> };
    const run = tool();
    try {
      return await run(call.tool, call.args(args as never));
    } catch (e) {
      if (isMissingThreadTool(e)) throw Failure.unsupported("This cua daemon has no Space thread; update Cua");
      throw e;
    }
  };
  return Object.fromEntries(THREAD_OPERATIONS.map((op) => [op, run(op)])) as Handlers;
}
