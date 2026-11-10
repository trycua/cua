// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The plan limit is "computers in use at once" everywhere a person reads it
// (CUA-3690). This scans the plan, usage and upgrade files so the old wording
// ("live sessions", "machines at once", "connections at once", "Machines in
// use") cannot come back. The refusal sentence itself comes from the Rust core,
// which has its own test; the files here must also not carry the old wording.
// The toast test keeps the old sentence on purpose (older relays still send it),
// so test files are not scanned.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const SRC = join(dirname(fileURLToPath(import.meta.url)), "..");
const PLAN_FILES = [
  "components/settings/account-tab.tsx",
  "i18n/locales/en/settings.json",
  "components/upgrade-card.tsx",
  "components/upsell-prompt.tsx",
  "bridge/adapters/demo/relay-usage.ts",
  "bridge/ops/relay-usage.ts",
];
const OLD_WORDS = [/live sessions/i, /machines at once/i, /connections at once/i, /machines in use/i];

describe("plan and usage copy", () => {
  it.each(PLAN_FILES)("%s never says the old words for the plan limit", (file) => {
    const text = readFileSync(join(SRC, file), "utf8");
    for (const word of OLD_WORDS) expect(text).not.toMatch(word);
  });

  it("the Settings usage row says computers in use", () => {
    const text = readFileSync(join(SRC, "i18n/locales/en/settings.json"), "utf8");
    expect(text).toContain("Computers in use right now");
    expect(text).toContain("computers in use at once");
  });
});
