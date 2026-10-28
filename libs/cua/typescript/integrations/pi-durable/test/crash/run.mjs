// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// Crash-injection runner. For each run: a fresh Space and storage; start the
// worker (agent.mjs), SIGKILL it at a random fault point (optionally
// restarting the Space's machine too), start it again, until the run
// settles. Then check the invariant: the Space's workspace equals what the
// committed transcript implies, every completed step applied exactly once and
// every interrupted one not at all.
//
// Usage: [CRASH_MODE=<mode>] node test/crash/run.mjs [runs=50] [steps=8] [out.jsonl]
//
// CRASH_MODE (default checkpoint):
//   checkpoint     a checkpoint of the Space after every call (the original campaign)
//   transactional  the call's diff committed with its outcome; no checkpoints
//   auto           transactional with the fall back to checkpoints
//   stale          two workers (epochs 1 and 2) on one log and Space: worker 1 is
//                  frozen mid call, worker 2 takes over, worker 1 is resumed and must
//                  stop with exit 75 without a single write landing (stale.mjs)
//   redrive        the coordinator's re-driver is the system under test: real
//                  session_start against the lab coordinator (CRASH_LAB_BIN), the
//                  driver killed at fault points, its Space killed, the coordinator
//                  killed and restarted (redrive.mjs; a spare Mac, see lab/README.md)
//   acp            the ACP durable driver (the cua-agents runner) against real harnesses
//                  (Claude Code, Codex, ...) with a mock model: kills of the runner, the
//                  harness and its container mid tool call, exactly-once of scripted effects
//                  (acp.mjs; a spare Mac with Docker, needs the mock model and the harness image)
// Every mode prints `SUMMARY <json>` (runs, ok, failed, kills, faults, recoveries,
// plus mode-specific numbers) and exits non-zero if any run failed.
//
// Other environment: CRASH_BACKEND=docker|microvm|local (local needs no Space: the
// cua-spacesd stand-in of test/local-space.mjs), CRASH_SEED_BASE (deterministic
// seeds), CRASH_EPOCHS=0 (run without epochs), CRASH_SNAPSHOTS=workspace|machine.
// docker needs the cua-pi-space image (see Dockerfile) and the cua SDK.

import { rmSync } from "node:fs"
import { BACKEND, SNAPSHOTS } from "./backend.mjs"
import { check, EPOCHS, finish, prepareRun, reportRun, seedBase } from "./campaign.mjs"
import { describe, latencyStats, newSummary, pick, startWorker } from "./common.mjs"
import { FAULTS_BY_MODE, MODES, RECOVERY_POINTS } from "./faults.mjs"

const MODE = process.env.CRASH_MODE ?? "checkpoint"
if (!MODES.includes(MODE)) {
  console.error(`CRASH_MODE must be one of ${MODES.join(", ")}, got ${MODE}`)
  process.exit(2)
}
const RUNS = Number(process.argv[2] ?? 50)
const STEPS = Number(process.argv[3] ?? 8)
const OUT = process.argv[4]
const MAX_STARTS = 40

console.log(`mode ${MODE}, backend ${BACKEND}, snapshots ${SNAPSHOTS}, epochs ${EPOCHS ? "on" : "off"}, seed base ${seedBase}`)

if (MODE === "stale") {
  const { campaign } = await import("./stale.mjs")
  finish(await campaign({ runs: RUNS, steps: STEPS, out: OUT, seedBase }), OUT)
}
if (MODE === "acp") {
  const { campaign } = await import("./acp.mjs")
  finish(await campaign({ runs: RUNS, steps: STEPS, out: OUT, seedBase }), OUT)
}
if (MODE === "redrive") {
  const { campaign } = await import("./redrive.mjs")
  finish(await campaign({ runs: RUNS, steps: STEPS, out: OUT, seedBase }), OUT)
}

// checkpoint, transactional, auto: kill the worker at fault points and restart it.
const FAULTS = FAULTS_BY_MODE[MODE]
const summary = newSummary({
  mode: MODE,
  backend: BACKEND,
  snapshots: SNAPSHOTS,
  epochs: EPOCHS,
  spaceRestarts: 0,
  spacesLost: 0,
})
// Resume latency: ms from a SIGKILL to the next worker's first MARK (the Space restart or rebuild, when the fault has one, is included).
const resumeMs = { none: [], restart: [], lost: [], all: [] }

for (let run = 0; run < RUNS; run++) {
  const ctx = await prepareRun(run, STEPS, MODE)
  const { random, env, chosen, space } = ctx
  const record = { run, seed: ctx.seed, mode: MODE, steps: STEPS, starts: 0, faults: [], recoveries: [], log: [], startedAt: Date.now() }
  try {
    await chosen.spaces.ensure(space)
    let last
    let previousKill
    for (;;) {
      if (++record.starts > MAX_STARTS) throw new Error("too many restarts")
      // Most starts get a fault; the last few run clean so every run settles.
      const faulty = record.starts < MAX_STARTS - 5 && random() < 0.85
      const fault = faulty
        ? (() => {
            // After an earlier kill, often aim at the recovery's own steps (a crash while recovering).
            const recovering = record.starts > 1 && random() < 0.35 ? FAULTS.filter(([p]) => RECOVERY_POINTS.includes(p)) : []
            // The first drive has no recovery to attack.
            const reachable = record.starts > 1 ? FAULTS : FAULTS.filter(([p]) => !RECOVERY_POINTS.includes(p))
            const [point, [lo, hi]] = pick(random, recovering.length > 0 ? recovering : reachable)
            return {
              point,
              occurrence: 1 + Math.floor(random() * 2),
              delay: Math.round(lo + random() * (hi - lo)),
              // The Space's machine restarts (processes die, disk stays), or
              // the Space is lost entirely and rebuilt from stored checkpoints.
              space: (() => {
                const r = random()
                return !chosen.spaceFaults ? undefined : r < 0.2 ? "restart" : r < 0.3 ? "lost" : undefined
              })(),
            }
          })()
        : undefined
      const result = await startWorker({ ...env, ...(EPOCHS ? { CRASH_EPOCH: String(record.starts) } : {}) }, fault, record.log)
      record.recoveries.push(...result.recoveries)
      if (previousKill && result.firstMarkAt) {
        const ms = result.firstMarkAt - previousKill.at
        resumeMs[previousKill.space ?? "none"].push(ms)
        resumeMs.all.push(ms)
        ;(record.resumeMs ??= []).push(ms)
      }
      previousKill = undefined
      if (result.killed) {
        previousKill = { at: result.killedAt, space: undefined }
        record.faults.push(fault)
        summary.kills++
        summary.faults[fault.point] = (summary.faults[fault.point] ?? 0) + 1
        previousKill.space = fault.space
        if (fault.space === "restart") {
          await chosen.restart(space)
          summary.spaceRestarts++
        } else if (fault.space === "lost") {
          await chosen.lose(space)
          summary.spacesLost++
        }
        continue
      }
      if (result.done) {
        last = result.done
        break
      }
      throw new Error(`worker exited ${result.code ?? result.signal} without settling: ${record.log.join("").slice(-2000)}${result.tail}`)
    }
    const checked = await check(ctx, STEPS)
    record.applied = checked.applied
    record.interrupted = checked.interrupted
    record.ok = checked.ok
    if (!checked.ok) record.mismatch = checked.mismatch
    // Transactional: the log's image of the workspace must equal the workspace too.
    if (MODE !== "checkpoint" && last?.includes("image=") && !last.includes("image=yes")) {
      record.ok = false
      record.mismatch = { ...(record.mismatch ?? {}), image: last }
    }
  } catch (error) {
    record.ok = false
    record.error = describe(error)
  }
  reportRun(summary, record, OUT)
  await chosen.cleanup(space)
  rmSync(ctx.dir, { recursive: true, force: true })
}

summary.resumeLatencyMs = Object.fromEntries(Object.entries(resumeMs).map(([k, v]) => [k, latencyStats(v)]))
finish(summary, OUT)
