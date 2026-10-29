// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's Settings → Connectors: the keys in memory, as the daemon
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

/** The demo's saved keys, shared with Connect (a sign-in Connect copies lands here, as in the daemon's store). */
export interface DemoKeyStore {
  keys: AgentKeyInfo[];
  /** Variables the provider refuses: a check marks them `expired` (needs a reconnect). `?demo=rejected-key`. */
  rejected: string[];
}

export const demoKeyStore = (now: () => number, rejected: string[] = [], claude?: "signed-in" | "expired" | "none"): DemoKeyStore => ({
  keys: [
    { provider: "openai", env: "OPENAI_API_KEY", last4: "3f9a", addedMs: now() - 6 * 24 * 3600_000 },
    // The sign-in imported from Claude Code (`?demo=claude-signed-in`), or one that ran out (`?demo=claude-expired`).
    ...(claude === "signed-in" || claude === "expired"
      ? [{ provider: "other" as const, env: "CLAUDE_CODE_OAUTH_TOKEN", last4: "", addedMs: now() - 26 * 3600_000, label: "Signed in as ada@example.com (Claude Max)", ...(claude === "expired" ? { expired: true } : {}) }]
      : []),
  ],
  rejected,
});

export function demoAgentKeysHandlers({
  wait,
  stepMs,
  now,
  store = demoKeyStore(now),
}: {
  wait(ms: number): Promise<void>;
  stepMs: number;
  now: () => number;
  store?: DemoKeyStore;
}): DemoHandlers<"agentKeys.list" | "agentKeys.check" | "agentKeys.set" | "agentKeys.remove" | "agentKeys.shell" | "agentKeys.fromShell"> {
  const keys = () => store.keys;
  const report = (): AgentKeysReport => ({ keys: keys().map((k) => ({ ...k })), providers: PROVIDERS, available: true, unavailable: null });
  return {
    "agentKeys.list": () => report(),
    // The providers answer: the keys they refuse need a reconnect, and a key saved again is not refused.
    "agentKeys.check": async () => {
      await wait(stepMs / 2);
      const r = report();
      return { ...r, keys: r.keys.map((k) => (store.rejected.includes(k.env) ? { ...k, expired: true } : k)) };
    },
    "agentKeys.set": async ({ provider, env, value }) => {
      const name = provider === "other" ? (env ?? "").trim() : PROVIDERS.find((p) => p.id === provider)?.env;
      if (!name) throw new HostError(`unknown provider ${provider}`);
      // The core's sheet refuses unsafe names before Save; the daemon checks again.
      if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) throw new HostError(`${name} is not an environment variable name`);
      const v = (value ?? "").trim();
      if (!v) throw new HostError("the key is empty");
      // The Keychain write.
      await wait(stepMs / 2);
      store.rejected = store.rejected.filter((e) => e !== name);
      store.keys = [...keys().filter((k) => k.env !== name), { provider: providerOf(name), env: name, last4: v.length >= 12 ? v.slice(-4) : "", addedMs: now() }].sort(
        (a, b) => a.env.localeCompare(b.env),
      );
      return report();
    },
    // The demo's shell exports an Anthropic key (`?demo=…` none needed): Use it saves a copy.
    "agentKeys.shell": async () => {
      await wait(stepMs);
      return { names: ["ANTHROPIC_API_KEY"] };
    },
    "agentKeys.fromShell": async ({ env }) => {
      if (env !== "ANTHROPIC_API_KEY") throw new HostError(`${env} isn't set in your shell`);
      await wait(stepMs / 2);
      store.keys = [...keys().filter((k) => k.env !== env), { provider: "anthropic", env, last4: "a91c", addedMs: now() }];
      return report();
    },
    "agentKeys.remove": async ({ env }) => {
      await wait(stepMs / 2);
      store.keys = keys().filter((k) => k.env !== env);
      return report();
    },
  };
}
