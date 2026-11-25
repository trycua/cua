#!/usr/bin/env node
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.
//
// The gRPC-Web client types (Connect-ES) are generated once, for the HTML5
// viewer, from libs/cua/proto (`cua-spacesd-html5/web`, `pnpm generate`).
// The phone uses the same files: this copies the services it calls
// (ComputerService, SystemService and what they import) into src/data/live/gen,
// and `--check` fails when the copy drifts from the viewer's.
import { cpSync, existsSync, mkdirSync, readdirSync, readFileSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const from = resolve(here, "../../../libs/cua/crates/cua-spacesd-html5/web/src/gen/cua/env/v1");
const to = resolve(here, "../src/data/live/gen/cua/env/v1");
const check = process.argv.includes("--check");
if (!existsSync(from)) {
  console.error(`missing ${from}`);
  process.exit(1);
}
if (check) {
  let bad = 0;
  for (const f of readdirSync(from)) {
    const a = readFileSync(join(from, f), "utf8");
    const p = join(to, f);
    if (!existsSync(p) || readFileSync(p, "utf8") !== a) {
      console.error(`drift: ${f}`);
      bad++;
    }
  }
  process.exit(bad ? 1 : 0);
}
rmSync(to, { recursive: true, force: true });
mkdirSync(to, { recursive: true });
cpSync(from, to, { recursive: true });
console.log("synced", readdirSync(to).length, "files");
