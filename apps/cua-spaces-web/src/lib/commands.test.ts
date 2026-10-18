// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { noCore, testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { filterCommands } from "./commands";

const cmds = [
  { group: "Go to", label: "Spaces", keywords: "" },
  { group: "Spaces", label: "Open QA on Tahoe", hint: "macOS 26", keywords: "macos running" },
  { group: "Spaces", label: "Stop QA on Tahoe", hint: "macOS 26", keywords: "macos running" },
  { group: "Actions", label: "Switch to dark appearance", keywords: "theme" },
  { group: "Keyvault", label: "Lock Keyvault", keywords: "secure" },
];

describe.skipIf(!wasmBuilt)("filterCommands (the core's ranking)", () => {
  it("matches every word across label, hint and keywords", async () => {
    const core = await testCore();
    expect(filterCommands(cmds, "tahoe running", core).map((c) => c.label)).toEqual(["Open QA on Tahoe", "Stop QA on Tahoe"]);
    expect(filterCommands(cmds, "THEME", core).map((c) => c.label)).toEqual(["Switch to dark appearance"]);
    expect(filterCommands(cmds, "  ", core)).toHaveLength(cmds.length);
  });

  it("matches loosely and ranks the best match first", async () => {
    const core = await testCore();
    expect(filterCommands(cmds, "stp tah", core).map((c) => c.label)).toEqual(["Stop QA on Tahoe"]);
    expect(filterCommands(cmds, "lock kv", core)[0]?.label).toBe("Lock Keyvault");
    expect(filterCommands(cmds, "spa", core)[0]?.label).toBe("Spaces");
  });

  it("keeps groups together", async () => {
    const core = await testCore();
    const groups = filterCommands(cmds, "a", core).map((c) => c.group);
    const seen: string[] = [];
    for (const g of groups) if (seen.at(-1) !== g) seen.push(g);
    expect(new Set(seen).size).toBe(seen.length);
  });
});

describe("filterCommands without the core", () => {
  it("keeps the commands that contain every word, in order", () => {
    expect(filterCommands(cmds, "tahoe stop", noCore).map((c) => c.label)).toEqual(["Stop QA on Tahoe"]);
    expect(filterCommands(cmds, "", noCore)).toHaveLength(cmds.length);
  });
});
