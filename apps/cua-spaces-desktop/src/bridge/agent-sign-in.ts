// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// An agent's own sign-in on another computer (CUA-3907): `agents.computerSignIn`,
// the daemon's person-only tool `agent_sign_in`. The arguments come from the web
// bridge's own `AGENT_SIGN_IN_TOOLS` and the answer is the tool's JSON as is
// (`ops/agent-sign-in.ts` maps it).

import { AGENT_SIGN_IN_TOOLS, isMissingSignInTool } from "../../../cua-spaces-web/src/bridge/ops/agent-sign-in";
import { string } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { daemonTool } from "./host-parts";

type Tool = (name: string, args?: Record<string, unknown>) => Promise<unknown>;

const ACTIONS = ["status", "start", "code", "cancel"];

/** `tool`: the daemon's tools (tests pass their own). */
export function agentSignInMethods(ctx: BridgeContext, tool: () => Tool = () => daemonTool(ctx, "Signing an agent in on another computer")): Handlers {
  return {
    "agents.computerSignIn": async (args: Record<string, unknown>) => {
      string(args, "space");
      const agent = string(args, "agent");
      if (agent !== "claude-code" && agent !== "openai-codex") throw Failure.badArgs("agent: claude-code or openai-codex");
      const action = typeof args.action === "string" ? args.action : "status";
      if (!ACTIONS.includes(action)) throw Failure.badArgs("action: status, start, code or cancel");
      if (action === "code") string(args, "code");
      const call = AGENT_SIGN_IN_TOOLS["agents.computerSignIn"];
      try {
        return await tool()(call.tool, call.args({ ...args, action } as never));
      } catch (e) {
        if (isMissingSignInTool(e)) throw Failure.unsupported("This cua daemon can't sign agents in on other computers; update Cua");
        throw e;
      }
    },
  };
}
