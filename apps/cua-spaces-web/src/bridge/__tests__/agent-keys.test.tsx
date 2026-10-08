// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { createDemoAdapter } from "../adapters/demo";
import type { CoreClient } from "../core";
import { BridgeProvider, useAgentKeys, type AgentKeysHook } from "../index";
import { agentKeysFromWire, tauriAgentKeysOps, webkitAgentKeysOps } from "../ops/agent-keys";
import { agentKeyForm, agentKeyNameProblemOf } from "../agent-keys";
import { replayFlow } from "../parity";
import { noCore, testCore, wasmBuilt } from "./testCore";

afterEach(cleanup);

const WIRE = {
  keys: [
    { provider: "anthropic", env: "ANTHROPIC_API_KEY", last4: "0000", added_ms: 1790000000000 },
    { provider: "other", env: "MISTRAL_API_KEY", last4: "9z9z", added_ms: 1790000200000, value: "must-not-pass" },
  ],
  providers: [
    { id: "anthropic", label: "Anthropic", env: "ANTHROPIC_API_KEY" },
    { id: "openai", label: "OpenAI", env: "OPENAI_API_KEY" },
  ],
  available: true,
};

describe("agent key operations", () => {
  it("reads the daemon's report and never keeps a value", () => {
    const r = agentKeysFromWire(WIRE);
    expect(r.keys).toEqual([
      { provider: "anthropic", env: "ANTHROPIC_API_KEY", last4: "0000", addedMs: 1790000000000 },
      { provider: "other", env: "MISTRAL_API_KEY", last4: "9z9z", addedMs: 1790000200000 },
    ]);
    expect(JSON.stringify(r)).not.toContain("must-not-pass");
    expect(r.available).toBe(true);
    const off = agentKeysFromWire({ keys: [], available: false, unavailable: "no Keychain" });
    expect(off).toMatchObject({ available: false, unavailable: "no Keychain" });
    expect(off.providers.map((p) => p.id)).toEqual(["anthropic", "openai"]);
    expect(agentKeysFromWire(null).keys).toEqual([]);
  });

  it("calls the daemon's app methods through Tauri's agents_tool", async () => {
    const calls: [string, Record<string, unknown> | undefined][] = [];
    const ops = tauriAgentKeysOps(async <T,>(cmd: string, args?: Record<string, unknown>) => {
      calls.push([cmd, args]);
      return WIRE as T;
    });
    await ops["agentKeys.list"]({});
    await ops["agentKeys.set"]({ provider: "anthropic", value: "sk-ant-test-0000", env: "IGNORED" });
    await ops["agentKeys.set"]({ provider: "other", value: "k", env: " MISTRAL_API_KEY " });
    await ops["agentKeys.remove"]({ env: "MISTRAL_API_KEY" });
    expect(calls).toEqual([
      ["agents_tool", { tool: "agent_keys.list", args: {} }],
      ["agents_tool", { tool: "agent_keys.set", args: { provider: "anthropic", value: "sk-ant-test-0000" } }],
      ["agents_tool", { tool: "agent_keys.set", args: { provider: "other", env: "MISTRAL_API_KEY", value: "k" } }],
      ["agents_tool", { tool: "agent_keys.remove", args: { env: "MISTRAL_API_KEY" } }],
    ]);
  });

  it("routes each operation to the SwiftUI host's method of the same name", async () => {
    const sent: [string, Record<string, unknown> | undefined][] = [];
    const ops = webkitAgentKeysOps(async <T,>(method: string, args?: Record<string, unknown>) => {
      sent.push([method, args]);
      return WIRE as T;
    });
    const r = await ops["agentKeys.remove"]({ env: "ANTHROPIC_API_KEY" });
    expect(sent).toEqual([["agentKeys.remove", { env: "ANTHROPIC_API_KEY" }]]);
    expect(r.keys).toHaveLength(2);
  });

  it("keeps keys in the demo host as the daemon describes them", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 2 });
    const first = await demo.call("agentKeys.list", {});
    expect(first.keys.map((k) => k.env)).toEqual(["OPENAI_API_KEY"]);
    const after = await demo.call("agentKeys.set", { provider: "anthropic", value: "sk-ant-test-0000" });
    expect(after.keys.find((k) => k.env === "ANTHROPIC_API_KEY")).toMatchObject({ provider: "anthropic", last4: "0000" });
    expect(JSON.stringify(after)).not.toContain("sk-ant-test");
    await expect(demo.call("agentKeys.set", { provider: "other", env: "MY KEY", value: "x" })).rejects.toThrow(/not an environment variable name/);
    const removed = await demo.call("agentKeys.remove", { env: "ANTHROPIC_API_KEY" });
    expect(removed.keys.map((k) => k.env)).toEqual(["OPENAI_API_KEY"]);
    demo.dispose?.();
  });
});

function mount(core: CoreClient) {
  const adapter = createDemoAdapter({ latencyMs: 1, stepMs: 4 });
  const hooks = {} as { keys: AgentKeysHook };
  function Probe() {
    hooks.keys = useAgentKeys();
    return null;
  }
  render(
    <BridgeProvider adapter={adapter} core={core}>
      <Probe />
    </BridgeProvider>,
  );
  return { hooks, adapter };
}

describe.skipIf(!wasmBuilt)("Settings → Agents through the core", () => {
  it("adds, replaces and removes a key through the hook", async () => {
    const { hooks, adapter } = mount(await testCore());
    await waitFor(() => expect(hooks.keys.view?.rows.map((r) => r.status)).toEqual(["Not set", "•••• 3f9a"]));
    hooks.keys.openSheet("anthropic");
    await waitFor(() => expect(hooks.keys.sheet).toEqual({ provider: "anthropic", env: null }));
    await hooks.keys.save("anthropic", "sk-ant-test-0000", "ANTHROPIC_API_KEY");
    await waitFor(() => expect(hooks.keys.view?.rows[0]).toMatchObject({ status: "•••• 0000", actionLabel: "Replace", removeLabel: "Remove" }));
    expect(hooks.keys.sheet).toBeNull();
    expect(hooks.keys.removeConfirmOf("ANTHROPIC_API_KEY")?.title).toBe("Remove the Anthropic key?");
    await hooks.keys.remove("ANTHROPIC_API_KEY");
    await waitFor(() => expect(hooks.keys.view?.rows[0]?.status).toBe("Not set"));
    expect(JSON.stringify(hooks.keys)).not.toContain("sk-ant-test");
    adapter.dispose?.();
  });

  it("asks for the variable of an Other key and refuses a loader's", async () => {
    const core = await testCore();
    const input = { keys: [] };
    const bad = agentKeyForm(core, input, { provider: "other", name: "LD_PRELOAD", hasValue: true })!;
    expect(bad.canSave).toBe(false);
    expect(bad.nameError).toMatch(/changes how programs run/);
    const ok = agentKeyForm(core, input, { provider: "other", name: "GEMINI_API_KEY", hasValue: true })!;
    expect(ok).toMatchObject({ canSave: true, env: "GEMINI_API_KEY", nameLabel: "Variable name" });
    for (const good of ["GEMINI_API_KEY", "groq_key", "_X"]) expect(agentKeyNameProblemOf(core, good), good).toBeNull();
    for (const name of ["", "1KEY", "MY KEY", "my-key", "PATH", "home", "DYLD_INSERT_LIBRARIES", "LD_PRELOAD", "CUA_HOME", "NODE_OPTIONS", "BASH_FUNC_x"]) {
      expect(agentKeyNameProblemOf(core, name), name).not.toBeNull();
    }
  });

  it("replays the agent-keys parity flow through the bridge", async () => {
    const core = await testCore();
    const replay = replayFlow(core, "agent-keys");
    expect(replay.bridged.sort()).toEqual(["agentKeys.form", "agentKeys.nameProblem", "agentKeys.removeConfirm", "agentKeys.view"]);
    expect(replay.transcript).toEqual(replay.golden);
  });
});

describe("Settings → Agents without the core", () => {
  it("draws nothing it would have to make up", () => {
    const { hooks, adapter } = mount(noCore);
    expect(hooks.keys.view).toBeNull();
    expect(agentKeyForm(noCore, { keys: [] }, { provider: "anthropic" })).toBeNull();
    adapter.dispose?.();
  });
});
