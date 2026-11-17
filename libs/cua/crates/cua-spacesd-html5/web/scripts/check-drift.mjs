#!/usr/bin/env node
// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Drift gate: the generated protobuf code (src/gen) and the embedded build
// (../assets) must match what `pnpm generate` and `pnpm build` produce.
import { spawnSync } from "node:child_process";

function run(cmd, args) {
  const r = spawnSync(cmd, args, { stdio: "inherit" });
  if (r.status !== 0) process.exit(r.status ?? 1);
}
run("pnpm", ["run", "generate"]);
run("pnpm", ["run", "build"]);
const diff = spawnSync("git", ["status", "--porcelain", "--", "src/gen", "../assets"], { encoding: "utf8" });
if (diff.stdout.trim()) {
  console.error("cua-spacesd-html5 drift: commit the regenerated files:\n" + diff.stdout);
  process.exit(1);
}
console.log("cua-spacesd-html5: generated code and assets are up to date");
