// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { desktopSpace, otherMachineCommand, otherMachinePrompt, OTHER_MACHINE_PERMISSIONS, OTHER_MACHINE_RELOAD } from "./other-machine";

describe("an agent on another machine", () => {
  it("signs in to the same account, then sets up the agent", () => {
    expect(otherMachineCommand("hermes")).toBe("cua auth login\ncua agents setup --agents hermes --yes");
    expect(otherMachineCommand("claude-code")).toBe("cua auth login\ncua agents setup --agents claude --yes");
  });

  it("names this machine's desktop Space by its relay id", () => {
    expect(desktopSpace("0123abcd")).toBe("relay:0123abcd");
    expect(otherMachinePrompt("Ada's Mac", "0123abcd")).toContain('space="relay:0123abcd"');
    // Before this machine is set up for access, the agent is told how to find it instead.
    expect(otherMachinePrompt("Ada's Mac", undefined)).toContain("list_spaces");
  });

  it("says what the machine must allow and that the agent needs a restart", () => {
    expect(OTHER_MACHINE_PERMISSIONS).toMatch(/share its desktop/);
    expect(OTHER_MACHINE_RELOAD).toMatch(/Restart the agent/);
    for (const text of [OTHER_MACHINE_PERMISSIONS, OTHER_MACHINE_RELOAD]) expect(text).not.toMatch(/·|device/i);
  });
});
