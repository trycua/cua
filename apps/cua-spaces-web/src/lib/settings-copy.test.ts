// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { plainDevice, plainDeviceSentence, plainKeysIntro } from "./settings-copy";

describe("plainDevice", () => {
  it.each([
    ["Enroll…", "Trust this machine"],
    ["Enrolled until Nov 18, 2026", "Trusted until Nov 18, 2026"],
    ["Re-verification due", "Needs a quick check"],
    ["macOS · Enrolled · re-verify in 24 days", "macOS · Trusted · check again in 24 days"],
    ["Revoke…", "Remove"],
    ["This Device", "This machine"],
    ["Your Devices", "Your machines"],
    ["Recent Access", "Recent activity"],
    ["New Computers", "Machines waiting for you"],
    ["Rename", "Rename"],
  ])("%s as %s", (from, to) => expect(plainDevice(from)).toBe(to));
});

describe("plainDeviceSentence", () => {
  it("rewrites a banner and a dialog", () => {
    expect(plainDeviceSentence("Enroll this device to reach your machines.")).toBe("Trust this machine to reach your machines.");
    expect(plainDeviceSentence("This device will no longer list or reach your machines until you enroll it again.")).toBe(
      "This machine will no longer list or reach your machines until you trust it again.",
    );
    expect(plainDeviceSentence("An unenrolled device opened mac-mini")).toBe("An untrusted device opened mac-mini");
  });
});

describe("plainKeysIntro", () => {
  it("says account key, and keeps the system's words about where keys stay", () => {
    const core = "Agents in your Spaces need a provider key to run. Keys stay on this PC in Windows Credential Manager, and each one is only given to the agents that use it.";
    expect(plainKeysIntro(core)).toBe(
      "Agents in your Spaces need an account key from their AI company to run. Keys stay on this PC in Windows Credential Manager, and each one is only given to the agents that use it.",
    );
  });
});
