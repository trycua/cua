// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import { agentKeyForm, agentKeysView } from "../agent-keys";
import { testCore, wasmBuilt } from "./testCore";

/** What Settings, Connectors says about where a saved key stays, per system. */
const WORDS = {
  macos: {
    intro: "Agents in your Spaces need a provider key to run. Keys stay on this Mac in the Keychain, and each one is only given to the agents that use it.",
    help: "It's saved in the Keychain on this Mac and won't be shown again.",
  },
  windows: {
    intro: "Agents in your Spaces need a provider key to run. Keys stay on this PC in Windows Credential Manager, and each one is only given to the agents that use it.",
    help: "It's saved in Windows Credential Manager on this PC and won't be shown again.",
  },
  // cua keeps no agent key in a file and has no vault on Linux: no store is named.
  linux: {
    intro: "Agents in your Spaces need a provider key to run. Keys stay on this machine, and each one is only given to the agents that use it.",
    help: "It's saved on this machine and won't be shown again.",
  },
} as const;

describe.skipIf(!wasmBuilt)("Settings, Connectors says where keys stay in the system's words", () => {
  it("has the core say it for each system, and the Mac's words by default", async () => {
    const core = await testCore();
    const input = { keys: [] };
    for (const os of ["macos", "windows", "linux"] as const) {
      expect(agentKeysView(core, input, os)!.intro, os).toBe(WORDS[os].intro);
      expect(agentKeyForm(core, input, { provider: "anthropic" }, os)!.valueHelp, os).toBe(WORDS[os].help);
    }
    // No system named: a Mac's, as every build said before.
    const bare = core.call<{ intro: string }>("agentKeys.view", { input });
    expect(bare.intro).toBe(WORDS.macos.intro);
    expect(core.call<{ valueHelp: string }>("agentKeys.form", { input, form: { provider: "openai" } }).valueHelp).toBe(WORDS.macos.help);
    // Only those two lines differ.
    const [mac, win] = [agentKeysView(core, input, "macos")!, agentKeysView(core, input, "windows")!];
    expect({ ...win, intro: "" }).toEqual({ ...mac, intro: "" });
    for (const os of ["windows", "linux"] as const) {
      const words = JSON.stringify([agentKeysView(core, { keys: [{ provider: "openai", env: "OPENAI_API_KEY", last4: "3f9a", addedMs: 1 }] }, os), agentKeyForm(core, input, { provider: "other" }, os)]);
      expect(words, os).not.toMatch(/Keychain|Mac/);
    }
  });
});
