// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings → Connections' provider keys (`agentKeys.*`), kept by the daemon in the
// system's credential store (the SwiftUI host's `AgentKeysModel`). The key
// travels once, in `agentKeys.set`'s arguments; answers never carry one.
import type { BridgeContext } from "./context";
import { optionalString, string } from "./args";
import type { Handlers } from "./host";

/** The non-secret choices a connection carries: a plan or region, and its fields (a model, a custom endpoint's URL). */
function choices(args: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  const region = optionalString(args, "region");
  if (region) out.region = region;
  const fields = args.fields;
  if (fields && typeof fields === "object" && !Array.isArray(fields)) {
    const clean = Object.fromEntries(Object.entries(fields as Record<string, unknown>).filter(([, v]) => typeof v === "string" && v.trim() !== ""));
    if (Object.keys(clean).length) out.fields = clean;
  }
  return out;
}

export function agentKeysMethods({ model }: BridgeContext): Handlers {
  return {
    "agentKeys.list": () => model.agents.keys.call("agent_keys.list"),
    "agentKeys.check": () => model.agents.keys.call("agent_keys.check"),
    "agentKeys.set": (args) => {
      // A custom endpoint may have no key: `value` is then empty.
      const call: Record<string, unknown> = { provider: string(args, "provider"), value: typeof args.value === "string" ? args.value : "" };
      const env = optionalString(args, "env");
      if (env) call.env = env;
      Object.assign(call, choices(args));
      // Another key beside the first one for the provider, named (it replaces nothing).
      if (args.add === true) call.add = true;
      const label = optionalString(args, "label");
      if (label) call.label = label;
      return model.agents.keys.call("agent_keys.set", call);
    },
    "agentKeys.remove": (args) => {
      const call: Record<string, unknown> = { env: string(args, "env") };
      const slot = optionalString(args, "slot");
      if (slot) call.slot = slot;
      if (args.keep_others === true) call.keep_others = true;
      return model.agents.keys.call("agent_keys.remove", call);
    },
    // Which saved credential an app that takes any provider uses from its next run (no `env`: the first that works).
    "agentKeys.use": (args) => {
      const call: Record<string, unknown> = { app: string(args, "app") };
      const env = optionalString(args, "env");
      if (env) call.env = env;
      const slot = optionalString(args, "slot");
      if (slot) call.slot = slot;
      return model.agents.keys.call("agent_keys.use", call);
    },
    // Which provider keys your login shell has (names only), and one saved from it (the value never crosses the bridge).
    "agentKeys.shell": () => model.agents.keys.call("agent_keys.shell"),
    "agentKeys.fromShell": (args) => model.agents.keys.call("agent_keys.from_shell", { env: string(args, "env"), ...choices(args) }),
  };
}
