// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// Seeds of the crash campaign that once found a bug stay in the suite.
//
// Checkpoint mode, local Space, seeds 5001, 5019, 5021: the driver of epoch N was killed
// while an earlier driver's interrupted restore was pending. `bind` had already raised the
// Space past the recorded epoch (N+1), then finishing the restore fenced the Space again at
// the intent's older epoch (N): the Space refused its own driver as stale (exit 75) and a
// re-driver would burn an attempt per round. The restore now uses the higher epoch.

import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { dirname, join } from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"

const pkg = join(dirname(fileURLToPath(import.meta.url)), "..")

for (const seed of [5001, 5019, 5021]) {
  test(`checkpoint campaign seed ${seed}: an interrupted restore is finished under the driver's own epoch`, () => {
    const result = spawnSync(process.execPath, ["test/crash/run.mjs", "1", "8"], {
      cwd: pkg,
      encoding: "utf8",
      timeout: 120_000,
      env: { ...process.env, CRASH_BACKEND: "local", CRASH_MODE: "checkpoint", CRASH_SEED_BASE: String(seed) },
    })
    assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`)
    assert.match(result.stdout, /"failed":0/)
  })
}

// Stale mode, local Space, seed 1303 (the CI subset's base 1300 + run 3): failed on a starved
// 2-core CI runner with "workspace differs from what the committed log implies", interrupted=1.
// Cause was the cua-spacesd stand-in (test/local-space.mjs), not pi-durable: it made a ledger
// entry's directory and wrote meta.json after, so the second worker's GetExecRecord could read a
// half-made entry ("Unexpected end of JSON input", the call failed, bash ran unrecorded). The entry
// is now claimed whole by a rename and the command records its own pid. The stand-in's ledger is
// unit-tested in local-space-ledger.test.mjs; this keeps the seed itself in the suite.
for (const seed of [1303]) {
  test(`stale campaign seed ${seed}: the second worker reads a whole ledger entry`, () => {
    const result = spawnSync(process.execPath, ["test/crash/run.mjs", "1", "6"], {
      cwd: pkg,
      encoding: "utf8",
      timeout: 120_000,
      env: { ...process.env, CRASH_BACKEND: "local", CRASH_MODE: "stale", CRASH_SEED_BASE: String(seed) },
    })
    assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`)
    assert.match(result.stdout, /"failed":0/)
  })
}

// Auto mode, local Space, seed 1101 (the CI subset's base 1100 + run 1): failed on main's CI (2-vCPU
// runner, four modes at once) with log.txt holding "s1" twice: three kills (the bash step's spawn, the
// undo's commit, the re-run), "applied=6, interrupted=0"; 15 of 100 runs at 0.5 CPU. Cause was a pi-durable
// bug: the driver that re-ran the call after the undo used the keys of the epoch the undo committed, but
// `bind` had raised its epoch past it, and it committed `started` with its own epoch and no key epoch, so
// the next driver looked the call up under the wrong key and ran the command again. The interleaving is
// built explicitly in crash-tx.test.mjs ("crash during the recovery's re-run ..."); this keeps the seed.
// Two stand-in races found while reproducing it (seeds 1303 and 1309 of stale mode) are built explicitly
// in local-space-ledger.test.mjs.
for (const seed of [1101]) {
  test(`auto campaign seed ${seed}: a re-run's key epoch survives a crash during the re-run`, () => {
    const result = spawnSync(process.execPath, ["test/crash/run.mjs", "1", "6"], {
      cwd: pkg,
      encoding: "utf8",
      timeout: 120_000,
      env: { ...process.env, CRASH_BACKEND: "local", CRASH_MODE: "auto", CRASH_SEED_BASE: String(seed) },
    })
    assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`)
    assert.match(result.stdout, /"failed":0/)
  })
}
