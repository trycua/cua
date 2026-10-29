// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The web app's copy of the registry (generated from the provider files) and
// the core's view of it say the same, and the key operations carry a plan and
// a custom endpoint's fields without a value ever coming back.

import { describe, expect, it } from "vitest";

import { connectorsInputOf, providerPicker, providerSheet } from "../connectors";
import { METHOD_KINDS, PROVIDER_REGISTRY } from "../generated/providers";
import { agentKeysFromWire, tauriAgentKeysOps, webkitAgentKeysOps } from "../ops/agent-keys";
import { testCore, wasmBuilt } from "./testCore";

describe.skipIf(!wasmBuilt)("the registry in the web app", () => {
  it("lists the providers the core lists, in the same order, and no others", async () => {
    const core = await testCore();
    const input = connectorsInputOf([], { keys: [] });
    const picker = providerPicker(core, input, null, "", true)!;
    expect(picker.items.map((i) => i.provider)).toEqual(PROVIDER_REGISTRY.map((p) => p.id));
    // Up front is the same eight the core says are the first tier.
    expect(PROVIDER_REGISTRY.filter((p) => p.tier === "top").map((p) => p.id)).toEqual(providerPicker(core, input, null, "", false)!.items.map((i) => i.provider));
  });

  it("offers only method kinds from the closed set, and a key way for every provider that takes a key", async () => {
    const core = await testCore();
    const input = connectorsInputOf([], { keys: [] });
    for (const p of PROVIDER_REGISTRY) {
      const sheet = providerSheet(core, input, p.id)!;
      for (const w of sheet.ways) expect([...METHOD_KINDS, "api_key"]).toContain(w.kind);
      if (p.keyEnv) expect(sheet.ways.some((w) => w.id === "key" || w.id === "endpoint")).toBe(true);
    }
  });
});

describe("agent key operations for connections", () => {
  const WIRE = {
    keys: [
      { provider: "zai", env: "GLM_API_KEY", last4: "7d1f", added_ms: 1, region: "coding-global", value: "must-not-pass" },
      { provider: "custom", env: "CUSTOM_ENDPOINT_API_KEY", last4: "", added_ms: 2, fields: { base_url: "http://127.0.0.1:1234/v1", model: "qwen", "bad key!": "x", wire: "line\nbreak" } },
    ],
    providers: [],
    available: true,
  };

  it("reads a plan and non-secret fields, drops anything else, never keeps a value", () => {
    const r = agentKeysFromWire(WIRE);
    expect(r.keys[0]).toEqual({ provider: "zai", env: "GLM_API_KEY", last4: "7d1f", addedMs: 1, region: "coding-global" });
    expect(r.keys[1]!.fields).toEqual({ base_url: "http://127.0.0.1:1234/v1", model: "qwen" });
    expect(JSON.stringify(r)).not.toContain("must-not-pass");
  });

  it("sends the plan and fields on both hosts, and nothing extra for a plain key", async () => {
    const seen: { cmd: string; args: unknown }[] = [];
    const invoke = async <T>(cmd: string, args?: Record<string, unknown>) => (seen.push({ cmd, args: args?.args }), { keys: [], providers: [], available: true } as T);
    const tauri = tauriAgentKeysOps(invoke);
    await tauri["agentKeys.set"]({ provider: "zai", value: "k", region: "coding-global" });
    await tauri["agentKeys.set"]({ provider: "custom", value: "", fields: { base_url: "http://h/v1", model: "m", wire: "openai-chat" } });
    await tauri["agentKeys.set"]({ provider: "deepseek", value: "k" });
    await tauri["agentKeys.fromShell"]({ env: "DEEPINFRA_API_KEY", fields: { model: "x/y" } });
    expect(seen.map((s) => s.args)).toEqual([
      { provider: "zai", value: "k", region: "coding-global" },
      { provider: "custom", value: "", fields: { base_url: "http://h/v1", model: "m", wire: "openai-chat" } },
      { provider: "deepseek", value: "k" },
      { env: "DEEPINFRA_API_KEY", fields: { model: "x/y" } },
    ]);
    const sent: unknown[] = [];
    const webkit = webkitAgentKeysOps(async <T>(_m: string, args?: Record<string, unknown>) => (sent.push(args), { keys: [], providers: [], available: true } as T));
    await webkit["agentKeys.set"]({ provider: "zai", value: "k", region: "global" });
    expect(sent).toEqual([{ provider: "zai", value: "k", region: "global" }]);
  });
});
