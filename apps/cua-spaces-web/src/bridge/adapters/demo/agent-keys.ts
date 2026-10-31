// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's Settings → Connections: the keys in memory, as the daemon
 * describes them (never a value). OpenAI starts saved, Anthropic not.
 */

import { HostError } from "../../adapter";
import { PROVIDER_REGISTRY } from "../../generated/providers";
import type { AgentKeyInfo, AgentKeyProvider, AgentKeyUse, AgentKeysReport } from "../../ops/agent-keys";
import type { DemoHandlers } from "./context";

/** The registry's providers that take a key, as the daemon lists them. */
const PROVIDERS = PROVIDER_REGISTRY.filter((p) => p.keyEnv).map((p) => ({ id: p.id, label: p.name, env: p.keyEnv }));

const providerOf = (env: string): AgentKeyProvider => PROVIDERS.find((p) => p.env === env)?.id ?? "other";

/** The demo's saved keys, shared with Connect (a sign-in Connect copies lands here, as in the daemon's store). */
export interface DemoKeyStore {
  keys: AgentKeyInfo[];
  /** Which saved credential each app that takes any provider uses (`agentKeys.use`). */
  uses: Record<string, AgentKeyUse>;
  /** Variables the provider refuses: a check marks them `expired` (needs a reconnect). `?demo=rejected-key`. */
  rejected: string[];
}

const DAY = 24 * 3600_000;

/** `keys: "several"`: a few providers connected, one with a key the provider refuses (with `rejected`), a custom endpoint. */
const SEVERAL = (now: () => number): AgentKeyInfo[] => [
  { provider: "anthropic", env: "ANTHROPIC_API_KEY", last4: "a91c", addedMs: now() - 9 * DAY },
  { provider: "openrouter", env: "OPENROUTER_API_KEY", last4: "cdef", addedMs: now() - 4 * DAY },
  { provider: "deepseek", env: "DEEPSEEK_API_KEY", last4: "9c2e", addedMs: now() - 2 * DAY },
  { provider: "custom", env: "CUSTOM_ENDPOINT_API_KEY", last4: "", addedMs: now() - DAY, fields: { base_url: "http://192.168.1.20:1234/v1", model: "qwen3-coder", wire: "openai-chat" } },
];

export const demoKeyStore = (now: () => number, rejected: string[] = [], claude?: "signed-in" | "expired" | "none", keys?: "none" | "several", chatgpt?: boolean): DemoKeyStore => ({
  keys: [
    ...(keys === "none" ? [] : keys === "several" ? SEVERAL(now) : [{ provider: "openai", env: "OPENAI_API_KEY", last4: "3f9a", addedMs: now() - 6 * DAY }]),
    // The sign-in imported from Claude Code (`?demo=claude-signed-in`), or one that ran out (`?demo=claude-expired`).
    ...(claude === "signed-in" || claude === "expired"
      ? [{ provider: "other" as const, env: "CLAUDE_CODE_OAUTH_TOKEN", last4: "", addedMs: now() - 26 * 3600_000, label: "Signed in as ada@example.com (Claude Max)", ...(claude === "expired" ? { expired: true } : {}) }]
      : []),
    // The ChatGPT sign-in made for Codex (`?demo=chatgpt-signed-in`): Codex's alone, never an API key.
    ...(chatgpt ? [{ provider: "other" as const, env: "CODEX_AUTH_JSON", last4: "", addedMs: now() - 3 * 3600_000, label: "Signed in with ChatGPT" }] : []),
  ],
  rejected,
  uses: {},
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
}): DemoHandlers<"agentKeys.list" | "agentKeys.check" | "agentKeys.set" | "agentKeys.remove" | "agentKeys.use" | "agentKeys.shell" | "agentKeys.fromShell"> {
  const keys = () => store.keys;
  const report = (): AgentKeysReport => ({ keys: keys().map((k) => ({ ...k })), ...(Object.keys(store.uses).length ? { uses: { ...store.uses } } : {}), providers: PROVIDERS, available: true, unavailable: null });
  return {
    "agentKeys.list": () => report(),
    // The providers answer: the keys they refuse need a reconnect, and a key saved again is not refused.
    "agentKeys.check": async () => {
      await wait(stepMs / 2);
      const r = report();
      return { ...r, keys: r.keys.map((k) => (store.rejected.includes(k.env) ? { ...k, expired: true } : k)) };
    },
    "agentKeys.set": async ({ provider, env, value, region, fields, add, label }) => {
      const name = provider === "other" ? (env ?? "").trim() : PROVIDERS.find((p) => p.id === provider)?.env;
      if (!name) throw new HostError(`unknown provider ${provider}`);
      // The core's sheet refuses unsafe names before Save; the daemon checks again.
      if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(name)) throw new HostError(`${name} is not an environment variable name`);
      const v = (value ?? "").trim();
      // A custom endpoint needs no key; every other provider does.
      if (!v && provider !== "custom") throw new HostError("the key is empty");
      if (provider === "custom" && !(fields?.base_url && fields?.model)) throw new HostError("a custom endpoint needs its base URL and model");
      // The Keychain write.
      await wait(stepMs / 2);
      store.rejected = store.rejected.filter((e) => e !== name);
      // Another key goes beside the first one (named); saving the first again replaces only it.
      const slot = add && keys().some((k) => k.env === name && !k.slot) ? `k${now().toString(16)}` : undefined;
      if (label && (label.length > 40 || /[\u0000-\u001f]/.test(label))) throw new HostError("a key's name must be one line of at most 40 characters");
      store.keys = [
        ...keys().filter((k) => !(k.env === name && !k.slot && !slot)),
        { provider: providerOf(name), env: name, ...(slot ? { slot } : {}), last4: v.length >= 12 ? v.slice(-4) : "", addedMs: now(), ...(label?.trim() ? { label: label.trim() } : {}), ...(region ? { region } : {}), ...(fields && Object.keys(fields).length ? { fields } : {}) },
      ].sort((a, b) => a.env.localeCompare(b.env) || (a.slot ?? "").localeCompare(b.slot ?? ""));
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
    "agentKeys.remove": async ({ env, slot, keepOthers }) => {
      await wait(stepMs / 2);
      if (slot) store.keys = keys().filter((k) => !(k.env === env && k.slot === slot));
      else if (keepOthers) {
        const rest = keys().filter((k) => !(k.env === env && !k.slot));
        const next = rest.filter((k) => k.env === env && k.slot).sort((x, y) => x.addedMs - y.addedMs)[0];
        // An app that was told to use the removed first key has no credential; it does not move onto the next.
        for (const u of Object.values(store.uses)) if (u.env === env && !u.slot) u.slot = "removed";
        store.keys = rest.map((k) => (k === next ? (({ slot: _slot, ...r }) => r)(k) : k));
      } else store.keys = keys().filter((k) => k.env !== env);
      return report();
    },
    // Which credential an app uses: one that is saved and that the app can read (the demo knows the apps that take any provider).
    "agentKeys.use": async ({ app, env, slot }) => {
      await wait(stepMs / 2);
      if (!["hermes", "opencode", "goose", "pi", "openclaw"].includes(app)) throw new HostError(`${app} reads one fixed sign-in; there is nothing to choose`);
      if (!env) {
        delete store.uses[app];
        return report();
      }
      const k = keys().find((x) => x.env === env && (x.slot ?? undefined) === (slot ?? undefined) && !x.expired);
      if (!k || env === "CLAUDE_CODE_OAUTH_TOKEN" || env === "CODEX_AUTH_JSON") throw new HostError("that key isn't saved, or has stopped working");
      store.uses[app] = { provider: k.provider, env: k.env, ...(k.slot ? { slot: k.slot } : {}) };
      return report();
    },
  };
}
