// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { appendFileSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { makeCore, setCoreForTests } from "../../src/core";

interface Fixture {
  method: string;
  args: unknown;
  result?: unknown;
  error?: string;
}

const canon = (v: unknown): string =>
  JSON.stringify(v, (_, x) =>
    x && typeof x === "object" && !Array.isArray(x) ? Object.fromEntries(Object.entries(x).sort(([a], [b]) => a.localeCompare(b))) : x,
  );

export const FIXTURES: Fixture[] = JSON.parse(readFileSync(join(__dirname, "../../core/fixtures/results.json"), "utf8"));

/** The core as the phone has it, answering from core/fixtures/results.json (which `cargo test --test fixtures` keeps equal to the real core). */
export function installFixtureCore(): void {
  const table = new Map(FIXTURES.map((f) => [`${f.method} ${canon(f.args)}`, f]));
  setCoreForTests(
    makeCore((method, args) => {
      const key = `${method} ${canon(JSON.parse(args))}`;
      if (!table.has(key) && process.env.RECORD_CORE_CALLS) {
        appendFileSync(join(__dirname, "../../core/fixtures/pending.ndjson"), `${JSON.stringify({ method, args: JSON.parse(args) })}\n`);
        return "null";
      }
      if (!table.has(key))
        throw new Error(`no fixture for ${method} ${args}: add it to core/fixtures/calls.json and run UPDATE_FIXTURES=1 cargo test --test fixtures in core/`);
      const f = table.get(key)!;
      if (f.error) throw new Error(f.error);
      return JSON.stringify(f.result);
    }),
  );
}

export function uninstallCore(): void {
  setCoreForTests(makeCore(null));
}
