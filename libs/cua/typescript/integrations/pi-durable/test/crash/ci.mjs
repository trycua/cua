// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// The fast subset of the crash campaign for CI (`npm run test:crash:ci`): no
// Docker and no lab. The modes that need only this machine run in parallel
// against the local cua-spacesd stand-in (real SQLite log, real child
// processes killed with SIGKILL or frozen with SIGSTOP); small run counts and
// deterministic seeds (CRASH_SEED_BASE, default 1000). The redrive mode needs
// the lab coordinator and real Spaces, so its CI coverage is the Rust
// session tests (`cargo test -p cua-spaces-cluster --test sessions`, run by the
// "cua SDK" workflow).
//
// Exit status is non-zero when any mode failed. Each mode's SUMMARY is printed.

import { spawn } from "node:child_process"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const run = join(dirname(fileURLToPath(import.meta.url)), "run.mjs")
const base = Number(process.env.CRASH_SEED_BASE ?? 1000)

const PLAN = [
  { mode: "transactional", runs: 12, steps: 6, seed: base },
  { mode: "auto", runs: 6, steps: 6, seed: base + 100 },
  { mode: "checkpoint", runs: 6, steps: 6, seed: base + 200 },
  { mode: "stale", runs: 10, steps: 6, seed: base + 300 },
]

const results = await Promise.all(
  PLAN.map(
    (p) =>
      new Promise((resolve) => {
        const child = spawn(process.execPath, [run, String(p.runs), String(p.steps)], {
          env: { ...process.env, CRASH_BACKEND: "local", CRASH_MODE: p.mode, CRASH_SEED_BASE: String(p.seed) },
          stdio: ["ignore", "pipe", "pipe"],
        })
        let out = ""
        child.stdout.on("data", (d) => (out += d))
        child.stderr.on("data", (d) => (out += d))
        child.on("exit", (code) => resolve({ ...p, code, out }))
      }),
  ),
)

let failed = 0
for (const r of results) {
  console.log(`--- ${r.mode} (seed base ${r.seed}) exit ${r.code}`)
  console.log(r.out.trimEnd())
  if (r.code !== 0) failed++
}
console.log(`CI SUBSET ${failed === 0 ? "passed" : `FAILED (${failed} of ${results.length} modes)`}`)
process.exit(failed === 0 ? 0 : 1)
