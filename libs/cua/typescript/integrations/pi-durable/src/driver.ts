// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * `runDurableSession`: the entry point of a durable agent session's driver
 * process, the thing a coordinator (re)launches for every attempt.
 *
 * The coordinator hands the attempt its identity in the environment:
 *
 * | variable | meaning |
 * | --- | --- |
 * | `CUA_SESSION` | session name |
 * | `CUA_SESSION_EPOCH` | this attempt's epoch (only grows; the fencing token) |
 * | `CUA_SESSION_ATTEMPT` | attempt number |
 * | `CUA_SESSION_RESUME` | `1` when a journal exists or the epoch is above 1 |
 * | `CUA_SESSION_SPACE` | the durable Space's name (may be empty) |
 * | `CUA_SESSION_SPACE_ADDR`, `_TOKEN`, `_LEASE` | how to reach that Space and its lease fence token (passed through) |
 * | `CUA_SESSION_STATE_DIR` | where the driver keeps its journal; empty: `$CUA_JOB_DATA/session` |
 * | `CUA_SESSION_HEARTBEAT_SECS` | print `CUA-HEARTBEAT <epoch> <unix_ms>` to stdout this often (0: do not); the coordinator cancels a driver whose output stops growing |
 *
 * and reads the outcome from the exit code: 0 finished, 78 permanent failure
 * (do not resume), 75 fenced (a newer epoch exists; ignore this process),
 * anything else crashed (resume with backoff).
 *
 * The driver opens the epoch-fenced log in the state directory (raising its
 * floor, which cuts off the previous driver's writes), raises the Space's
 * epoch, then hands both to the caller's `start` to build the Harness (model
 * and provider wiring stay with the caller) and waits for the run to settle.
 * Everything else lives in the library.
 */

import { mkdirSync } from "node:fs"
import { join } from "node:path"
import type { DurableSpaces } from "./durable.js"
import { type Fence, haltOf, type HaltError, isStaleEpochError, openFencedSqlite, type FencedSqlite, staleEpochOf, type StaleEpochError } from "./fence.js"

export const EXIT_DONE = 0
export const EXIT_PERMANENT = 78
export const EXIT_FENCED = 75
export const EXIT_CRASHED = 1

/** Throw this from `start`/`wait` for a failure that resuming cannot fix (bad configuration, rejected credentials). Exit 78. */
export class PermanentFailure extends Error {
  override readonly name = "PermanentFailure"
}

/** The attempt's identity, from the environment contract. */
export interface SessionInfo {
  name: string
  epoch: number
  attempt: number
  /** Whether to continue an existing journal instead of starting the session. */
  resume: boolean
  space: string | undefined
  spaceAddr: string | undefined
  spaceToken: string | undefined
  spaceLease: string | undefined
  stateDir: string
  /** Seconds between heartbeat lines (0: none). */
  heartbeatSecs: number
}

/** Parses the launch contract; throws `PermanentFailure` when it is unusable. */
export function readSessionEnv(env: NodeJS.ProcessEnv = process.env): SessionInfo {
  const name = env.CUA_SESSION
  if (!name) throw new PermanentFailure("CUA_SESSION is not set")
  const epoch = Number(env.CUA_SESSION_EPOCH)
  if (!Number.isSafeInteger(epoch) || epoch < 1) throw new PermanentFailure(`CUA_SESSION_EPOCH must be a positive integer, got ${JSON.stringify(env.CUA_SESSION_EPOCH)}`)
  const attempt = Number(env.CUA_SESSION_ATTEMPT ?? 1)
  let stateDir = env.CUA_SESSION_STATE_DIR
  if (!stateDir && env.CUA_JOB_DATA) stateDir = join(env.CUA_JOB_DATA, "session")
  if (!stateDir) throw new PermanentFailure("neither CUA_SESSION_STATE_DIR nor CUA_JOB_DATA is set")
  const optional = (value: string | undefined) => (value ? value : undefined)
  return {
    name,
    epoch,
    attempt: Number.isFinite(attempt) ? attempt : 1,
    resume: env.CUA_SESSION_RESUME === "1",
    space: optional(env.CUA_SESSION_SPACE),
    spaceAddr: optional(env.CUA_SESSION_SPACE_ADDR),
    spaceToken: optional(env.CUA_SESSION_SPACE_TOKEN),
    spaceLease: optional(env.CUA_SESSION_SPACE_LEASE),
    stateDir,
    heartbeatSecs: Math.max(0, Number(env.CUA_SESSION_HEARTBEAT_SECS ?? 0) || 0),
  }
}

/**
 * Prints `CUA-HEARTBEAT <epoch> <unix_ms>` now and then every `secs` seconds,
 * from a timer of its own: a frozen process stops it (so the coordinator sees
 * silence and replaces the driver), a busy agent does not. Returns the stop function.
 */
export function startHeartbeat(epoch: number, secs: number, write: (line: string) => void = (line) => void process.stdout.write(line)): () => void {
  if (!(secs > 0)) return () => {}
  const beat = () => write(`CUA-HEARTBEAT ${epoch} ${Date.now()}\n`)
  beat()
  const timer = setInterval(beat, secs * 1000)
  timer.unref()
  return () => clearInterval(timer)
}

/** What `start` receives. */
export interface DriverContext {
  session: SessionInfo
  /** The epoch-fenced log: pass `storage` to `Harness.open`. */
  log: FencedSqlite
  /** The log's fence: pass to `durableSpaces({ fence })`. */
  fence: Fence
  /** Pass to `durableSpaces({ onStale })` so a refusal by the Space halts the driver at once. */
  onStale: (error: StaleEpochError) => void
  /** Pass to `durableSpaces({ onHalt })`: a failure to establish the epoch fence stops the driver and its log writes at once. */
  onHalt: (error: HaltError) => void
}

/** A started session: resolves when its submission settles. */
export interface DriverRun {
  /** `done` is success; `unanswered` ends the session without an answer (permanent by default). */
  wait(): Promise<{ status: string }>
  /** Release the Harness and Space pipes. Called once, whatever the outcome. */
  close(): Promise<void>
  /**
   * Called after the run settled and before the driver reports `done`:
   * make the final workspace match the log, normally
   * `() => durable.ensureSpace(harness, root.id, context)`, which rebuilds a Space
   * lost after the last call.
   */
  ensureSpace?(): Promise<void>
}

export interface DurableSessionOptions {
  /** Default `process.env`. */
  env?: NodeJS.ProcessEnv
  /** The `durableSpaces()` of this process, to raise the Space's epoch before anything runs. */
  durable?: DurableSpaces
  /**
   * Builds the Harness over `context.log.storage`, resumes it (continuing the journal
   * when `context.session.resume`, otherwise submitting the session's input) and returns the run.
   */
  start(context: DriverContext): Promise<DriverRun>
  /** Maps a settled status to an exit code. Default: `done` is 0, everything else 78. */
  exitCode?: (status: string) => number
  /** Progress and failures, for the job log. */
  log?: (line: string) => void
  /** Where heartbeat lines go. Default: stdout. */
  heartbeat?: (line: string) => void
}

/** Runs one attempt and returns the process exit code (the caller calls `process.exit`). */
export async function runDurableSession(options: DurableSessionOptions): Promise<number> {
  const log = options.log ?? (() => {})
  let run: DriverRun | undefined
  let log_: FencedSqlite | undefined
  let stopBeat = () => {}
  try {
    const session = readSessionEnv(options.env)
    stopBeat = startHeartbeat(session.epoch, session.heartbeatSecs, options.heartbeat)
    mkdirSync(session.stateDir, { recursive: true })
    // Opening raises the log's floor to this epoch: an older driver's next commit is refused.
    log_ = await openFencedSqlite(join(session.stateDir, "log.db"), session.epoch)
    log(`session ${session.name} attempt ${session.attempt} epoch ${session.epoch}${session.resume ? " (resume)" : ""}`)
    let halt: (error: StaleEpochError) => void = () => {}
    const halted = new Promise<never>((_, reject) => {
      halt = reject
    })
    halted.catch(() => {})
    const stop = (error: Error) => {
      // Later commits by Pi (which would record the failure as a result) are refused.
      log_?.fence.halt(error)
      halt(error as StaleEpochError)
    }
    log_.fence.onStale((error) => halt(error))
    if (options.durable && session.space) await options.durable.raiseEpoch(session.space)
    run = await Promise.race([
      options.start({ session, log: log_, fence: log_.fence, onStale: (error) => halt(error), onHalt: stop }),
      halted,
    ])
    const settled = await Promise.race([run.wait(), halted])
    if (run.ensureSpace) await Promise.race([run.ensureSpace(), halted])
    const code = (options.exitCode ?? ((status) => (status === "done" ? EXIT_DONE : EXIT_PERMANENT)))(settled.status)
    log(`session settled: ${settled.status} -> exit ${code}`)
    return code
  } catch (error) {
    if (isStaleEpochError(error)) {
      log(`fenced: ${staleEpochOf(error)?.message}`)
      return EXIT_FENCED
    }
    const halt = haltOf(error)
    if (halt) {
      log(`halted: ${halt.message}`)
      return halt.permanent ? EXIT_PERMANENT : EXIT_CRASHED
    }
    if (error instanceof PermanentFailure) {
      log(`permanent failure: ${error.message}`)
      return EXIT_PERMANENT
    }
    log(`crashed: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}`)
    return EXIT_CRASHED
  } finally {
    stopBeat()
    try {
      await run?.close()
    } catch {
      // Nothing to add to the outcome.
    }
    await log_?.database.close().catch(() => {})
  }
}
