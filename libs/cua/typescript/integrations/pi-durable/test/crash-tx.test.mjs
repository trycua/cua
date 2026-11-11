// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// Crash at every point of a transactional-mode call: a driver process is
// SIGKILLed right after a chosen step, a new driver with the next epoch takes
// over the same log and Space (as a coordinator's re-driver would), and the
// finished workspace must equal an uninterrupted run's: every step applied
// exactly once, and equal to the image in the log.

import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { mkdirSync, mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join } from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"
import { workspaceListing } from "./local-space.mjs"

const here = dirname(fileURLToPath(import.meta.url))
const STEPS = 6

/** Starts a driver; kills it right after the `occurrence`-th `point` mark (after `delay` ms) when given a fault. */
function drive(dir, cwd, epoch, fault, extraEnv = {}) {
  return new Promise((resolve) => {
    const child = spawn(process.execPath, [join(here, "tx-agent.mjs")], {
      env: { ...process.env, TX_DIR: dir, TX_CWD: cwd, TX_EPOCH: String(epoch), TX_STEPS: String(STEPS), ...extraEnv },
      stdio: ["ignore", "pipe", "pipe"],
    })
    let seen = 0
    let killed = false
    let done
    let buffer = ""
    const trail = []
    // A driver that neither finishes nor dies is a bug; keep what it printed.
    const watchdog = setTimeout(() => {
      trail.push("WATCHDOG: killed after 45s")
      child.kill("SIGKILL")
    }, 45_000)
    const recoveries = []
    const errors = []
    child.stdout.on("data", (chunk) => {
      buffer += chunk.toString("utf8")
      let newline
      while ((newline = buffer.indexOf("\n")) !== -1) {
        const line = buffer.slice(0, newline)
        buffer = buffer.slice(newline + 1)
        trail.push(line)
        if (line.startsWith("DONE")) done = line
        else if (line.startsWith("RECOVERY ")) recoveries.push(JSON.parse(line.slice(9)))
        else if (fault && line === `MARK ${fault.point}` && ++seen === fault.occurrence && !killed) {
          setTimeout(() => {
            if (child.exitCode === null) {
              killed = true
              child.kill("SIGKILL")
            }
          }, fault.delay)
        }
      }
    })
    child.stderr.on("data", (chunk) => errors.push(chunk.toString("utf8")))
    child.on("exit", (code, signal) => {
      clearTimeout(watchdog)
      resolve({ code, signal, killed, done, recoveries, stderr: errors.join("") + trail.slice(-15).join("\n"), hung: trail.includes("WATCHDOG: killed after 45s") })
    })
  })
}

/** Runs the plan to completion, crashing per `faults` (one per start), and returns the end state. */
async function crashRun(faults, extraEnv) {
  const dir = mkdtempSync(join(tmpdir(), "pdv2-crash-"))
  const cwd = join(dir, "ws")
  mkdirSync(cwd)
  const recoveries = []
  let kills = 0
  for (let start = 1; start < 30; start++) {
    const result = await drive(dir, cwd, start, faults[start - 1], extraEnv)
    recoveries.push(...result.recoveries)
    if (result.killed) kills++
    if (result.done) {
      return { listing: workspaceListing(cwd), done: result.done, recoveries, kills, starts: start }
    }
    if (result.hung) assert.fail(`driver ${start} hung:\n${result.stderr}`)
    if (!result.killed) assert.fail(`driver ${start} exited ${result.code}/${result.signal} without finishing:\n${result.stderr}`)
  }
  assert.fail("did not finish in 30 starts")
}

const control = await crashRun([])

const POINTS = ["call", "started", "exec-spawn", "exec-running", "exec-exit", "executed", "outcome", "open"]

for (const point of POINTS) {
  for (const occurrence of point === "open" ? [1] : [1, 2, 3, 5]) {
    test(`crash after ${point} #${occurrence}: the finished workspace equals an uninterrupted run's and the log's image`, async () => {
      const run = await crashRun([{ point, occurrence, delay: occurrence % 2 ? 0 : 15 }])
      assert.ok(run.done.includes("image=yes"), run.done)
      assert.ok(run.done.includes("errors=0"), `no step ended as an error: ${run.done}`)
      assert.deepEqual(run.listing, control.listing)
    })
  }
}

test("crash again while recovering (after the undo is committed, after it ran): still exactly once", async () => {
  const run = await crashRun([
    { point: "executed", occurrence: 2, delay: 0 },
    { point: "undo-committed", occurrence: 1, delay: 0 },
    { point: "reverted", occurrence: 1, delay: 0 },
    { point: "started", occurrence: 1, delay: 0 },
  ])
  assert.ok(run.done.includes("image=yes"), run.done)
  assert.deepEqual(run.listing, control.listing)
  assert.ok(run.kills >= 2)
})

test("recovery reports the mode it ran in", async () => {
  const run = await crashRun([{ point: "executed", occurrence: 1, delay: 0 }, { point: "executed", occurrence: 1, delay: 0 }])
  assert.ok(run.recoveries.length > 0)
  assert.ok(run.recoveries.every((e) => e.mode === "transactional"), JSON.stringify(run.recoveries))
})

// Campaign seed 1101 (auto mode, CI subset; reproduced at 0.5 CPU in 15 of 100 runs): the plan's first bash
// step was cut off while it ran, the next driver decided to undo it (committing the new epoch E), was killed,
// and the driver after it found the Space already raised past E (its own session epoch is below the
// recorded one, so `bind` takes E+1). It re-ran the step under the key of epoch E (the daemon replays a
// key whatever epoch the request carries), but committed `started` with epoch E+1 and no key epoch, so the
// driver after that looked the step up under E+1, found nothing, and ran it a second time while the first
// re-run's command was still going: `echo s0 >> log.txt` twice. The key epoch is now committed with the
// re-run. The step sleeps long enough that the command is still running at every start, which makes the
// interleaving the same on every machine.
test("crash during the recovery's re-run after the Space was raised past the undo's epoch: the step still runs once", async () => {
  const slow = { TX_SLEEP: "2" }
  const clean = await crashRun([], slow)
  const run = await crashRun(
    [
      { point: "exec-running", occurrence: 1, delay: 0 },
      { point: "undo-committed", occurrence: 1, delay: 0 },
      { point: "exec-running", occurrence: 1, delay: 0 },
    ],
    slow,
  )
  assert.ok(run.kills >= 3, `the three kills landed: ${run.kills}`)
  assert.ok(run.done.includes("image=yes"), run.done)
  assert.deepEqual(run.listing, clean.listing)
})
