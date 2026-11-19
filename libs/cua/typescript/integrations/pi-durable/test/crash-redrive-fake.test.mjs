// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// The redrive mode's runner logic, against test/crash/fake-lab.mjs (a stand-in for
// the lab coordinator that runs each drive as a local process): drivers that
// SIGKILL themselves are resumed with the next epoch, the cap ends a session
// `failed`/exhausted, and the runner's invariants (drives = kills + 1, epochs
// increase, workspace == log) hold. The real re-driver is tested by the Rust
// session tests and, in real Spaces, by `npm run test:crash:redrive` on the lab.

import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"

const pkg = join(dirname(fileURLToPath(import.meta.url)), "..")

function redrive(flavors, runs, seed, extraEnv = {}) {
  const home = mkdtempSync(join(tmpdir(), "pdv2-fakelab-"))
  const result = spawnSync(process.execPath, ["test/crash/run.mjs", String(runs), "6"], {
    cwd: pkg,
    encoding: "utf8",
    timeout: 120_000,
    env: {
      ...process.env,
      CRASH_BACKEND: "local",
      CRASH_MODE: "redrive",
      CRASH_REDRIVE_FLAVORS: flavors,
      CRASH_SEED_BASE: String(seed),
      CRASH_LAB_BIN: `${process.execPath} ${join(pkg, "test/crash/fake-lab.mjs")}`,
      CRASH_LAB_ARGS: "",
      CRASH_LAB_HOME: home,
      CRASH_DRIVER_DIR: pkg,
      CRASH_DRIVER_ARGV: JSON.stringify([process.execPath, join(pkg, "test/crash/agent.mjs")]),
      CRASH_REDRIVE_TIMEOUT_S: "90",
      CRASH_STALL_SECS: "4",
      ...extraEnv,
    },
  })
  const line = /^SUMMARY (.*)$/m.exec(result.stdout)
  assert.ok(line, `no SUMMARY:\n${result.stdout}\n${result.stderr}`)
  return { status: result.status, summary: JSON.parse(line[1]), out: result.stdout }
}

test("redrive: killed drivers are resumed (drives = kills + 1), the cap ends a session failed/exhausted", () => {
  const { status, summary, out } = redrive("kill,exhaust", 2, 700)
  assert.equal(status, 0, out)
  assert.equal(summary.ok, 2, out)
  assert.equal(summary.failed, 0)
  assert.ok(summary.kills >= 3, "faults landed")
  assert.equal(summary.exhausted, 1)
  assert.equal(summary.spuriousResumes, 0)
  assert.equal(summary.epochViolations, 0)
  assert.equal(summary.overlapViolations, 0)
  assert.ok(summary.resumeLatencyMs.count >= 1 && summary.resumeLatencyMs.p50 > 0)
})

test("redrive: the coordinator killed during a backoff and restarted still finishes the session", () => {
  const { status, summary, out } = redrive("coordinator", 1, 702)
  assert.equal(status, 0, out)
  assert.equal(summary.coordinatorRestarts, 1)
  assert.equal(summary.ok, 1)
})

test("redrive: a frozen driver (SIGSTOP) is ended by stall detection, not a timeout, and resumed once", () => {
  const { status, summary, out } = redrive("hang", 1, 703)
  assert.equal(status, 0, out)
  assert.equal(summary.ok, 1, out)
  assert.ok(summary.hangs >= 1, "a freeze landed")
  assert.equal(summary.stalls, summary.hangs, "every freeze ended as a stall")
  assert.equal(summary.spuriousResumes, 0)
  assert.equal(summary.overlapViolations, 0)
})

// The lease flavor against the stand-in's durable table (a fencing counter, one holder): the runner's lease invariants.
for (const [sub, seed] of [["kill", 710], ["coordinator", 711], ["space", 712], ["race", 713]]) {
  test(`redrive lease (${sub}): epoch = max(previous + 1, lease token), superseded tokens refused, one holder`, () => {
    const { status, summary, out } = redrive("lease", 1, seed, { CRASH_LEASE_SUBS: sub, CRASH_FAKE_BACKOFF_MS: "1000" })
    assert.equal(status, 0, out)
    assert.equal(summary.ok, 1, out)
    assert.equal(summary.leaseViolations, 0, out)
    assert.equal(summary.epochViolations, 0)
    assert.equal(summary.overlapViolations, 0)
    assert.ok(summary.staleTokensRefused >= 1, "a superseded lease token was presented and refused")
    if (sub === "race") assert.equal(summary.raceWaits, 1, "the second session waited with a reason")
    if (sub === "space") assert.equal(summary.spaceReplaced, 1, "the Space was lost and placed again")
  })
}
