// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

import assert from "node:assert/strict"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import {
  EXIT_CRASHED,
  EXIT_DONE,
  EXIT_FENCED,
  EXIT_PERMANENT,
  openFencedSqlite,
  PermanentFailure,
  readSessionEnv,
  runDurableSession,
  startHeartbeat,
  StaleEpochError,
} from "../dist/index.js"

function env(extra = {}) {
  return { CUA_SESSION: "s1", CUA_SESSION_EPOCH: "2", CUA_SESSION_ATTEMPT: "2", CUA_SESSION_RESUME: "1", CUA_SESSION_STATE_DIR: mkdtempSync(join(tmpdir(), "pdv2-drv-")), ...extra }
}
const run = (status) => async () => ({ wait: async () => ({ status }), close: async () => {} })

test("the env contract is read, with the job data fallback and the lease passthrough", () => {
  const info = readSessionEnv(env({ CUA_SESSION_SPACE: "sp", CUA_SESSION_SPACE_ADDR: "direct:h:1", CUA_SESSION_SPACE_TOKEN: "t", CUA_SESSION_SPACE_LEASE: "9" }))
  assert.deepEqual([info.name, info.epoch, info.attempt, info.resume, info.space, info.spaceAddr, info.spaceToken, info.spaceLease], ["s1", 2, 2, true, "sp", "direct:h:1", "t", "9"])
  const fallback = readSessionEnv({ CUA_SESSION: "s", CUA_SESSION_EPOCH: "1", CUA_SESSION_STATE_DIR: "", CUA_JOB_DATA: "/data" })
  assert.equal(fallback.stateDir, "/data/session")
  assert.equal(fallback.resume, false)
  assert.equal(fallback.space, undefined)
  assert.throws(() => readSessionEnv({ CUA_SESSION: "s", CUA_SESSION_EPOCH: "0", CUA_SESSION_STATE_DIR: "/x" }), PermanentFailure)
})

test("exit codes: done 0, unanswered 78, permanent failure 78, stale 75, anything else 1", async () => {
  assert.equal(await runDurableSession({ env: env(), start: run("done") }), EXIT_DONE)
  assert.equal(await runDurableSession({ env: env(), start: run("unanswered") }), EXIT_PERMANENT)
  assert.equal(await runDurableSession({ env: env(), start: run("unanswered"), exitCode: () => 3 }), 3)
  assert.equal(await runDurableSession({ env: env(), start: async () => { throw new PermanentFailure("bad key") } }), EXIT_PERMANENT)
  assert.equal(await runDurableSession({ env: env({ CUA_SESSION_EPOCH: "x" }), start: run("done") }), EXIT_PERMANENT)
  assert.equal(await runDurableSession({ env: env(), start: async () => { throw new Error("boom") } }), EXIT_CRASHED)
  assert.equal(await runDurableSession({ env: env(), start: async () => { throw new StaleEpochError(1, 2, "space") } }), EXIT_FENCED)
})

test("a log already at a newer epoch fences the driver before it starts anything", async () => {
  const e = env()
  await (await openFencedSqlite(join(e.CUA_SESSION_STATE_DIR, "log.db"), 5)).database.close()
  let started = false
  const code = await runDurableSession({ env: e, start: async () => ((started = true), run("done")()) })
  assert.equal(code, EXIT_FENCED)
  assert.equal(started, false)
})

test("a newer driver opening the log while this one runs ends it with 75, even if the run never settles", async () => {
  const e = env()
  const path = join(e.CUA_SESSION_STATE_DIR, "log.db")
  const code = await runDurableSession({
    env: e,
    start: async ({ log }) => ({
      wait: () => new Promise(() => {}),
      close: async () => {},
      // The takeover happens while the run is going; its next commit is refused.
      ...(void (async () => {
        await new Promise((r) => setTimeout(r, 50))
        const newer = await openFencedSqlite(path, 3)
        await newer.database.close()
        await log.fence.check().catch(() => {})
      })()),
    }),
  })
  assert.equal(code, EXIT_FENCED)
})

test("onStale from the Space halts the driver the same way", async () => {
  const code = await runDurableSession({
    env: env(),
    start: async ({ onStale }) => ({
      wait: () => new Promise(() => setTimeout(() => onStale(new StaleEpochError(2, 3, "space")), 20)),
      close: async () => {},
    }),
  })
  assert.equal(code, EXIT_FENCED)
})

test("close runs on every outcome", async () => {
  let closed = 0
  const make = (wait) => async () => ({ wait, close: async () => void closed++ })
  await runDurableSession({ env: env(), start: make(async () => ({ status: "done" })) })
  await runDurableSession({ env: env(), start: make(async () => { throw new Error("x") }) })
  assert.equal(closed, 2)
})

test("ensureSpace runs after the run settles and before the driver reports done; its failure is a crash", async () => {
  const order = []
  const make = (ensure) => async () => ({
    wait: async () => (order.push("wait"), { status: "done" }),
    ensureSpace: ensure,
    close: async () => {},
  })
  assert.equal(await runDurableSession({ env: env(), start: make(async () => void order.push("ensure")) }), EXIT_DONE)
  assert.deepEqual(order, ["wait", "ensure"])
  assert.equal(await runDurableSession({ env: env(), start: make(async () => { throw new Error("space gone") }) }), EXIT_CRASHED)
})

test("the heartbeat is printed at once and then on a timer of its own, and stops with the driver", async () => {
  const lines = []
  const e = env({ CUA_SESSION_HEARTBEAT_SECS: "1" })
  assert.equal(readSessionEnv(e).heartbeatSecs, 1)
  const code = await runDurableSession({
    env: e,
    heartbeat: (line) => lines.push(line),
    start: async () => ({ wait: () => new Promise((resolve) => setTimeout(() => resolve({ status: "done" }), 2300)), close: async () => {} }),
  })
  assert.equal(code, EXIT_DONE)
  assert.ok(lines.length >= 3, `expected at least 3 beats, got ${lines.length}`)
  assert.match(lines[0], /^CUA-HEARTBEAT 2 \d{13}\n$/)
  const n = lines.length
  await new Promise((resolve) => setTimeout(resolve, 1300))
  assert.equal(lines.length, n, "no beats after the driver ended")
  // Off by default and when 0.
  const none = []
  await runDurableSession({ env: env(), heartbeat: (l) => none.push(l), start: run("done") })
  assert.deepEqual(none, [])
  assert.equal(readSessionEnv(env({ CUA_SESSION_HEARTBEAT_SECS: "x" })).heartbeatSecs, 0)
  const stop = startHeartbeat(1, 0, (l) => none.push(l))
  stop()
  assert.deepEqual(none, [])
})
