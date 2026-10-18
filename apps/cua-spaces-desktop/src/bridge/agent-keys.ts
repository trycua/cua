// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings → Agents' provider keys (`agentKeys.*`), kept by the daemon in the
// system's credential store (the SwiftUI host's `AgentKeysModel`). The key
// travels once, in `agentKeys.set`'s arguments; answers never carry one.
import type { BridgeContext } from "./context";
import { optionalString, string } from "./args";
import type { Handlers } from "./host";

export function agentKeysMethods({ model }: BridgeContext): Handlers {
  return {
    "agentKeys.list": () => model.agents.keys.call("agent_keys.list"),
    "agentKeys.set": (args) => {
      const call: Record<string, unknown> = { provider: string(args, "provider"), value: string(args, "value") };
      const env = optionalString(args, "env");
      if (env) call.env = env;
      return model.agents.keys.call("agent_keys.set", call);
    },
    "agentKeys.remove": (args) => model.agents.keys.call("agent_keys.remove", { env: string(args, "env") }),
  };
}
