// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// CRASH_MODE=stale: a driver that is not dead but frozen, then wakes up after a
// newer one took over (a partitioned or paused host). Per run, on one fenced
// log and one Space:
//
//   1. worker 1 (epoch 1) is SIGSTOPped at a random fault marker;
//   2. worker 2 (epoch 2) starts, takes over the log and the Space (their floors
//      rise to 2, the Space kills epoch 1's commands) and works on;
//   3. worker 1 is SIGCONTed (right after the takeover, a few marks later, or after
//      worker 2 finished);
//      A worker 1 frozen INSIDE a SQLite write transaction holds the log's lock, so
//      worker 2 cannot open the log until worker 1 runs again (counted as
//      `frozenInTransaction`): the runner then moves the freeze to just outside a
//      transaction. Should a takeover still be blocked (`blockedTakeovers`), worker 1 is
//      woken, the floor was never raised, so it may legitimately finish; only
//      consistency is checked for that run.
//   4. worker 1 must exit 75 with `STALE`, and nothing it did after waking may land:
//      when worker 2 had already finished, the committed log and the workspace are
//      byte-for-byte what worker 2 left; always, workspace == what the log implies,
//      every step exactly once.
//
// The durable mode (transactional or checkpoint) is drawn per run.

import { createHash } from "node:crypto"
import { execFileSync } from "node:child_process"
import { rmSync, writeFileSync } from "node:fs"
import { DatabaseSync } from "node:sqlite"
import { check, prepareRun, reportRun, seedBase, spaceListing } from "./campaign.mjs"
import { committedMessages, describe, latencyStats, newSummary, pick, spawnWorker } from "./common.mjs"
import { FAULTS_BY_MODE, RECOVERY_POINTS } from "./faults.mjs"

const WAIT_MS = 60_000

/** Whether another process holds the log's write lock right now. */
function writeLocked(path) {
  const db = new DatabaseSync(path, { timeout: 0 })
  try {
    db.exec("BEGIN IMMEDIATE")
    db.exec("ROLLBACK")
    return false
  } catch {
    return true
  } finally {
    db.close()
  }
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

function until(promise, ms, what) {
  let timer
  return Promise.race([
    promise,
    new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`timed out after ${ms}ms waiting for ${what}`)), ms)
    }),
  ]).finally(() => clearTimeout(timer))
}

/** A digest of everything worker 1 could have changed: the committed transcript and the workspace. */
async function fingerprint(ctx) {
  const messages = await committedMessages(ctx.db)
  const listing = await spaceListing(ctx.chosen, ctx.space, ctx.work)
  return createHash("sha256").update(JSON.stringify(messages)).update(listing).digest("hex")
}

export async function campaign({ runs, steps, out }) {
  const summary = newSummary({ mode: "stale", frozen: {}, woke: {}, rejected: { log: 0, space: 0, other: 0 }, wokeAfterDone: 0, strictChecks: 0 })
  const wakeLatency = []
  for (let run = 0; run < runs; run++) {
    // The mode alternates by seed so a small fixed run count covers both.
    const mode = process.env.CRASH_STALE_MODE ?? ((seedBase + run) % 2 === 0 ? "transactional" : "checkpoint")
    const ctx = await prepareRun(run, steps, mode)
    const { random, env, chosen, space } = ctx
    // Points every call passes through on a first drive (not the recovery steps).
    const faults = FAULTS_BY_MODE[mode === "transactional" ? "stale" : "checkpoint"].filter(([p]) => !RECOVERY_POINTS.includes(p))
    const [point, [lo, hi]] = pick(random, faults)
    const freeze = { point, occurrence: 1 + Math.floor(random() * 2), delay: Math.round(lo + random() * (hi - lo)) }
    // When worker 1 wakes: as soon as worker 2 is in, a few marks later, or after worker 2 is done.
    const wake = pick(random, ["takeover", "later", "done"])
    const record = { run, seed: ctx.seed, mode: "stale", durable: mode, steps, freeze, wake, faults: [freeze], recoveries: [], log: [], startedAt: Date.now() }
    const children = []
    try {
      await chosen.spaces.ensure(space)
      // 1. Worker 1 until the freeze.
      let seen = 0
      let frozen
      const frozenSignal = new Promise((resolve) => {
        frozen = resolve
      })
      const first = spawnWorker({ ...env, CRASH_EPOCH: "1" }, {
        log: record.log,
        onLine(mark, child) {
          if (mark.point === freeze.point && ++seen === freeze.occurrence && !frozen.done) {
            frozen.done = true
            setTimeout(() => {
              if (child.exitCode === null) {
                child.kill("SIGSTOP")
                frozen(true)
              } else frozen(false)
            }, freeze.delay)
          }
        },
      })
      children.push(first.child)
      const reached = await until(Promise.race([frozenSignal, first.exited.then(() => false)]), WAIT_MS, `worker 1 to reach ${freeze.point}`)
      if (!reached) {
        // The plan had no such point (for example fewer exec'd steps than the occurrence asks for): worker 1 ran
        // to the end alone. Not a stale case; it still has to be consistent. Counted so it cannot hide.
        const ended = await first.exited
        summary.unreached = (summary.unreached ?? 0) + 1
        record.unreached = true
        const checked = await check(ctx, steps)
        record.applied = checked.applied
        record.interrupted = checked.interrupted
        record.ok = ended.code === 0 && ended.done !== undefined && checked.ok
        if (!record.ok) record.error = `worker 1 ended ${ended.code ?? ended.signal} before the freeze point and the run is inconsistent: ${ended.tail}`
        throw Object.assign(new Error("unreached"), { handled: true })
      }
      // A worker frozen INSIDE a write transaction holds the log's lock for as long as it is frozen
      // (see above). Count those, then move the freeze to just outside a transaction (wake it for a
      // few ms and stop it again), so the takeover is attempted against a lock that is free, and
      // the frozen worker is not cut off from its Space long enough to time its connections out.
      let moved = 0
      while (writeLocked(ctx.db) && moved < 200) {
        if (moved++ === 0) summary.frozenInTransaction = (summary.frozenInTransaction ?? 0) + 1
        first.child.kill("SIGCONT")
        await sleep(3 + Math.floor(random() * 10))
        first.child.kill("SIGSTOP")
        await sleep(2)
      }
      summary.kills++
      summary.frozen[point] = (summary.frozen[point] ?? 0) + 1
      summary.faults[point] = (summary.faults[point] ?? 0) + 1

      // 2. Worker 2 takes over.
      let marks2 = 0
      let takeover
      const tookOver = new Promise((resolve) => {
        takeover = resolve
      })
      const second = spawnWorker({ ...env, CRASH_EPOCH: "2" }, {
        log: record.log,
        onLine(mark) {
          // `call` is the first mark after the Space's epoch was raised (agent.mjs raises it right after `open`).
          if (mark.point === "call" || ++marks2 > 4) takeover(true)
        },
      })
      children.push(second.child)
      const second_exit = second.exited.then((r) => {
        takeover(false)
        return r
      })
      const took = await until(tookOver, WAIT_MS, "worker 2 to take over")
      if (!took) {
        const blocked = await second_exit
        if (!/database is locked/.test(record.log.join("")) || blocked.code === 0) {
          throw new Error(`worker 2 exited ${blocked.code ?? blocked.signal} before taking over: ${record.log.join("").slice(-1500)}${blocked.tail}`)
        }
        // Frozen inside a write transaction: the lock outlives the freeze. Wake worker 1, then a
        // retry with the next epoch (what a coordinator does after the crash) finishes the session.
        summary.blockedTakeovers = (summary.blockedTakeovers ?? 0) + 1
        record.blockedTakeover = true
        record.log.length = 0
        first.child.kill("SIGCONT")
        const woken = await until(first.exited, WAIT_MS, "worker 1 after a blocked takeover")
        const third = spawnWorker({ ...env, CRASH_EPOCH: "3" }, { log: record.log })
        children.push(third.child)
        const finished = await until(third.exited, 4 * WAIT_MS, "the retry to finish")
        const problems = []
        if (woken.code !== 0 && woken.code !== 75) problems.push(`worker 1 exited ${woken.code ?? woken.signal} after waking: ${woken.tail}`)
        if (woken.code === 75) summary.rejected[/resource=(\S+)/.exec(woken.stale ?? "")?.[1] === "space" ? "space" : "log"]++
        if (!finished.done) problems.push(`the retry exited ${finished.code ?? finished.signal} without settling: ${finished.tail}`)
        const checked = await check(ctx, steps)
        record.applied = checked.applied
        record.interrupted = checked.interrupted
        if (!checked.ok) {
          problems.push("workspace differs from what the committed log implies")
          record.mismatch = checked.mismatch
        }
        record.ok = problems.length === 0
        if (!record.ok) record.error = problems.join("; ")
        throw Object.assign(new Error("blocked"), { handled: true })
      }

      // 3. Worker 1 wakes up.
      let atDone
      if (wake === "later") await sleep(Math.round(random() * 400))
      if (wake === "done") {
        const settled = await until(second_exit, 4 * WAIT_MS, "worker 2 to finish")
        if (!settled.done) throw new Error(`worker 2 exited ${settled.code ?? settled.signal} without settling: ${settled.tail}`)
        atDone = await fingerprint(ctx)
        summary.wokeAfterDone++
      }
      const wokeAt = Date.now()
      first.child.kill("SIGCONT")
      const woke = await until(first.exited, Number(process.env.CRASH_WAKE_MS ?? WAIT_MS), "worker 1 to stop after waking").catch((error) => {
        if (process.env.CRASH_PS) {
          try {
            const ps = execFileSync("ps", ["-o", "pid,state,pcpu,rss,command", "-p", children.map((c) => c.pid).join(",")], { encoding: "utf8" })
            writeFileSync(process.env.CRASH_PS, ps)
          } catch {}
        }
        if (process.env.CRASH_SAMPLE) {
          try {
            execFileSync("sample", [String(first.child.pid), "1", "-file", process.env.CRASH_SAMPLE], { stdio: "ignore" })
          } catch {}
        }
        throw new Error(`${error.message}; it printed: ${first.state.lines.slice(-12).join(" | ")}; stderr: ${record.log.join("").slice(-800)}`)
      })
      wakeLatency.push(Date.now() - wokeAt)
      summary.woke[wake] = (summary.woke[wake] ?? 0) + 1
      const finished = await until(second_exit, 4 * WAIT_MS, "worker 2 to finish")

      // 4. The invariants.
      const problems = []
      if (woke.code !== 75) problems.push(`worker 1 exited ${woke.code ?? woke.signal}, not 75 (it kept going after being fenced): ${woke.tail}`)
      if (woke.done) problems.push(`worker 1 settled the session while stale: ${woke.done}`)
      if (!woke.stale) problems.push("worker 1 did not report STALE")
      else {
        const resource = /resource=(\S+)/.exec(woke.stale)?.[1]
        summary.rejected[resource === "log" || resource === "space" ? resource : "other"]++
      }
      if (!finished.done) problems.push(`worker 2 exited ${finished.code ?? finished.signal} without settling: ${finished.tail}`)
      if (atDone !== undefined) {
        summary.strictChecks++
        if ((await fingerprint(ctx)) !== atDone) problems.push("worker 1 changed the log or the workspace after waking (fingerprint differs from worker 2's final state)")
      }
      const checked = await check(ctx, steps)
      record.applied = checked.applied
      record.interrupted = checked.interrupted
      if (!checked.ok) {
        problems.push("workspace differs from what the committed log implies")
        record.mismatch = checked.mismatch
      }
      record.recoveries.push(...second.state.recoveries)
      record.stale = woke.stale
      record.ok = problems.length === 0
      if (!record.ok) record.error = problems.join("; ")
    } catch (error) {
      if (!error.handled) {
        record.ok = false
        record.error = describe(error)
      }
    } finally {
      for (const child of children) {
        if (child.exitCode === null) {
          child.kill("SIGCONT")
          child.kill("SIGKILL")
        }
      }
    }
    reportRun(summary, record, out)
    await chosen.cleanup(space)
    rmSync(ctx.dir, { recursive: true, force: true })
  }
  summary.wakeToExitMs = latencyStats(wakeLatency)
  return summary
}
