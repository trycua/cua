// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The first-run parity flow (cua-spaces-app-core/parity/first-run.json),
// replayed on the wasm core through the calls the page makes
// (`firstRunView`, `firstRunTodos`), must equal its golden.

import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import type { FirstRunInput } from "@/bridge/contracts/first-run";
import { firstRunTodos, firstRunView } from "@/bridge/first-run";
import type { HostOs } from "@/bridge/host-os";
import { firstSpaceOffers } from "@/bridge/new-space";
import type { WizardEnv } from "@/bridge/contracts/new-space";

const DIR = join(import.meta.dirname, "..", "..", "..", "..", "libs", "cua", "crates", "cua-spaces-app-core", "parity");

describe.skipIf(!wasmBuilt)("first-run parity (wasm core)", () => {
  it("replays the flow through the page's calls and matches the golden", async () => {
    const core = await testCore();
    const flow = readFileSync(join(DIR, "first-run.json"), "utf8");
    const golden = JSON.parse(readFileSync(join(DIR, "golden", "first-run.json"), "utf8")) as unknown;
    const out = core.parity!.run("first-run", flow, (method, args) => {
      // A machine's one-click offers, as the page reads them.
      if (method === "wizard.firstSpaceOffers") {
        const a = args as { env: WizardEnv; hostOs: HostOs };
        return firstSpaceOffers(core, a.env, a.hostOs);
      }
      const input = (args as { input: FirstRunInput }).input;
      if (method === "firstRun.view") return firstRunView(core, input);
      if (method === "firstRun.todos") return firstRunTodos(core, input);
      throw new Error(`unexpected core call ${method}`);
    });
    expect(out).toEqual(golden);
  });
});
