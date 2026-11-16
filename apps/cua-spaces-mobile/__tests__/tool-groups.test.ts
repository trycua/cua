// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { core } from "../src/core";
import { installFixtureCore, uninstallCore } from "./helpers/core";

/**
 * The tool-call group header is the app core's `toolRows.label` (the phrase table of cua-agents' `tool_row::group_label`
 * now lives in the core; `cargo test -p cua-spaces-app-core` holds it equal to the crate for every category and to this
 * shared fixture). Here the phone reads the same cases through its entry point.
 */
const shared = JSON.parse(readFileSync(join(__dirname, "../../../libs/cua/crates/cua-agents/tests/fixtures/tool-rows.json"), "utf8")) as {
  groups: { cats: string[]; label: string | null }[];
};

beforeAll(installFixtureCore);
afterAll(uninstallCore);

describe("the tool-call group header against cua-agents' shared fixture", () => {
  it.each(shared.groups.map((g) => [JSON.stringify(g.cats), g] as const))("%s", (_n, g) => {
    expect(core.call("toolRows.label", { cats: g.cats })).toBe(g.label);
  });
});
