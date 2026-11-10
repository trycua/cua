#!/usr/bin/env node
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.
//
// `RECORD_CORE_CALLS=1 pnpm test` writes the core calls the tests make that
// core/fixtures/calls.json does not list yet into core/fixtures/pending.ndjson.
// This merges them in (deduplicated, in a stable order); then
// `UPDATE_FIXTURES=1 cargo test --test fixtures` in core/ computes the answers.
import { existsSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const dir = resolve(dirname(fileURLToPath(import.meta.url)), "../core/fixtures");
const pending = join(dir, "pending.ndjson");
if (!existsSync(pending)) {
  console.log("nothing pending");
  process.exit(0);
}
const calls = JSON.parse(readFileSync(join(dir, "calls.json"), "utf8"));
const canon = (v) =>
  JSON.stringify(v, (_, x) =>
    x && typeof x === "object" && !Array.isArray(x) ? Object.fromEntries(Object.entries(x).sort(([a], [b]) => a.localeCompare(b))) : x,
  );
const seen = new Set(calls.map((c) => `${c.method} ${canon(c.args)}`));
let added = 0;
for (const line of readFileSync(pending, "utf8").split("\n").filter(Boolean)) {
  const c = JSON.parse(line);
  const k = `${c.method} ${canon(c.args)}`;
  if (!seen.has(k)) {
    seen.add(k);
    calls.push(c);
    added++;
  }
}
writeFileSync(join(dir, "calls.json"), JSON.stringify(calls, null, 2) + "\n");
rmSync(pending);
console.log(`added ${added} calls`);
