// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's Settings → Agents: the keys in memory, as the daemon
 * describes them (never a value). OpenAI starts saved, Anthropic not.
 */

import { HostError } from "../../adapter";
import type { AgentKeyInfo, AgentKeyProvider, AgentKeysReport } from "../../ops/agent-keys";
import type { DemoHandlers } from "./context";

const PROVIDERS = [
  { id: "anthropic" as const, label: "Anthropic", env: "ANTHROPIC_API_KEY" },
  { id: "openai" as const, label: "OpenAI", env: "OPENAI_API_KEY" },
];

const providerOf = (env: string): AgentKeyProvider => PROVIDERS.find((p) => p.env === env)?.id ?? "other";

export function demoAgentKeysHandlers({ wait, stepMs, now }: { wait(ms: number): Promise<void>; stepMs: number; now: () => number }): DemoHandlers<
  "agentKeys.list" | "agentKeys.set" | "agentKeys.remove"
> {
  let keys: AgentKeyInfo[] = [{ provider: "openai", env: "OPENAI_API_KEY", last4: "3f9a", addedMs: now() - 6 * 24 * 3600_000 }];
  const report = (): AgentKeysReport => ({ keys: keys.map((k) => ({ ...k })), providers: PROVIDERS, available: true, unavailable: null });
  return {
    "agentKeys.list": () => report(),
    "agentKeys.set": async ({ provider, env, value }) => {
      const name = provider === "other" ? (env ?? "").trim() : PROVIDERS.find((p) => p.id === provider)?.env;
      if (!name) throw new HostError(`unknown provider ${provider}`);
      // The core's sheet refuses unsafe names before Save; the daemon checks again.
      if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) throw new HostError(`${name} is not an environment variable name`);
      const v = (value ?? "").trim();
      if (!v) throw new HostError("the key is empty");
      // The Keychain write.
      await wait(stepMs / 2);
      keys = [...keys.filter((k) => k.env !== name), { provider: providerOf(name), env: name, last4: v.length >= 12 ? v.slice(-4) : "", addedMs: now() }].sort(
        (a, b) => a.env.localeCompare(b.env),
      );
      return report();
    },
    "agentKeys.remove": async ({ env }) => {
      await wait(stepMs / 2);
      keys = keys.filter((k) => k.env !== env);
      return report();
    },
  };
}
