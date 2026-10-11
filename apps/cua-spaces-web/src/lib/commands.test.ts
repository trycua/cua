// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { filterCommands, fuzzyScore } from "./commands";

const cmds = [
  { group: "Go to", label: "Spaces", keywords: "" },
  { group: "Spaces", label: "Open QA on Tahoe", hint: "macOS 26", keywords: "macos running" },
  { group: "Spaces", label: "Stop QA on Tahoe", hint: "macOS 26", keywords: "macos running" },
  { group: "Actions", label: "Switch to dark appearance", keywords: "theme" },
  { group: "Keyvault", label: "Lock Keyvault", keywords: "secure" },
];

describe("fuzzyScore", () => {
  it("matches characters in order and prefers word starts and runs", () => {
    expect(fuzzyScore("Lock Keyvault", "lkv")).toBeGreaterThan(0);
    expect(fuzzyScore("Lock Keyvault", "vkl")).toBe(0);
    expect(fuzzyScore("Spaces", "spa")).toBeGreaterThan(fuzzyScore("Settings and spare", "spa"));
    expect(fuzzyScore("Keyvault", "key")).toBeGreaterThan(fuzzyScore("Keyvault", "kvt"));
  });
});

describe("filterCommands", () => {
  it("matches every word across label, hint and keywords", () => {
    expect(filterCommands(cmds, "tahoe running").map((c) => c.label)).toEqual(["Open QA on Tahoe", "Stop QA on Tahoe"]);
    expect(filterCommands(cmds, "THEME").map((c) => c.label)).toEqual(["Switch to dark appearance"]);
    expect(filterCommands(cmds, "  ")).toHaveLength(cmds.length);
  });

  it("matches loosely and ranks the best match first", () => {
    expect(filterCommands(cmds, "stp tah").map((c) => c.label)).toEqual(["Stop QA on Tahoe"]);
    expect(filterCommands(cmds, "lock kv")[0]?.label).toBe("Lock Keyvault");
    expect(filterCommands(cmds, "spa")[0]?.label).toBe("Spaces");
  });

  it("keeps groups together", () => {
    const groups = filterCommands(cmds, "a").map((c) => c.group);
    const seen: string[] = [];
    for (const g of groups) if (seen.at(-1) !== g) seen.push(g);
    expect(new Set(seen).size).toBe(seen.length);
  });
});
