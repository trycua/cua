// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// What every mode of the crash campaign shares: per-run setup for the chosen
// backend, the consistency check, and the per-run and final reports.

import { randomBytes } from "node:crypto"
import { appendFileSync, mkdirSync, mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { sh } from "../../dist/spacesd.js"
import { BACKEND, backend } from "./backend.mjs"
import { committedMessages, seeded, tally } from "./common.mjs"
import { implied, isInterrupted, listing } from "./plan.mjs"

export const DEFAULT_WORK = process.env.CRASH_WORK ?? (BACKEND === "microvm" ? "/home/cua/work" : "/work")
export const EPOCHS = process.env.CRASH_EPOCHS !== "0"
export const seedBase = Number(process.env.CRASH_SEED_BASE ?? Date.now() % 100000)
const token = randomBytes(16).toString("hex")
process.env.CRASH_TOKEN = token

/**
 * What one run needs, per backend: its scratch directory, workspace path,
 * environment for its workers and the backend to drive. `local` runs get a
 * private Space (and workspace) in the directory.
 */
export async function prepareRun(run, steps, durableMode) {
  const seed = seedBase + run
  const space = `crash-${seedBase}-${run}`
  const dir = mkdtempSync(join(tmpdir(), "pi-crash-"))
  const work = BACKEND === "local" ? join(dir, "ws") : DEFAULT_WORK
  if (BACKEND === "local") {
    mkdirSync(work, { recursive: true })
    process.env.CRASH_LOCAL_DIR = dir
  }
  const chosen = await backend()
  const env = {
    ...process.env,
    CRASH_DB: join(dir, "session.sqlite"),
    CRASH_SEED: String(seed),
    CRASH_STEPS: String(steps),
    CRASH_SPACE: space,
    CRASH_TOKEN: token,
    CRASH_WORK: work,
    CRASH_STORE: join(dir, "checkpoints"),
    CRASH_DURABLE_MODE: durableMode,
    // Docker Spaces need no cua home; microVM Spaces live in the cua daemon's.
    ...(BACKEND === "docker" ? { CUA_HOME: join(dir, "cua-home"), CUA_TEST: "1" } : {}),
    ...(BACKEND === "local" ? { CRASH_LOCAL_DIR: dir } : {}),
  }
  return { seed, space, dir, work, db: env.CRASH_DB, env, chosen, random: seeded(seed) }
}

/** The Space's workspace as a `sha256sum`-style listing. */
export async function spaceListing(chosen, name, work) {
  if (chosen.listing) return chosen.listing(name, work)
  // A fresh connection: the worker may have replaced the Space (restore).
  chosen.spaces.invalidate?.(name)
  const spacesd = await chosen.spaces.spacesd(name)
  const result = await sh(spacesd, `cd ${work} && find . -type f -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum`)
  if (result.code !== 0) throw new Error(`listing failed: ${result.stderr}`)
  return result.stdout.trim()
}

/** The consistency check: workspace == what the committed log implies, every step once. */
export async function check(ctx, steps) {
  const messages = await committedMessages(ctx.db)
  const { files, applied, failed } = implied(messages)
  const expected = listing(files)
  const actual = await spaceListing(ctx.chosen, ctx.space, ctx.work)
  return {
    interrupted: messages.filter(isInterrupted).length,
    applied: applied.length,
    ok: expected === actual && applied.length === steps && failed.length === 0,
    mismatch:
      expected === actual && applied.length === steps && failed.length === 0
        ? undefined
        : { expected, actual, applied: applied.length, failed },
  }
}

export function reportRun(summary, record, out) {
  record.ms = Date.now() - record.startedAt
  delete record.startedAt
  tally(summary, record)
  if (record.ok) delete record.log
  else record.log = Array.isArray(record.log) ? record.log.join("").slice(-4000) : record.log
  console.log(
    `run ${record.run} seed ${record.seed}: ${record.ok ? "ok" : "FAILED"} starts=${record.starts ?? "?"} kills=${record.faults?.length ?? record.kills ?? 0} ` +
      `interrupted=${record.interrupted ?? "?"} ${record.ms}ms${record.error ? ` error=${record.error.split("\n")[0]}` : ""}`,
  )
  if (!record.ok) {
    // A CI log is all there is to go on after a failure: show what differed and the last of the log.
    if (record.mismatch) console.log(`  mismatch ${JSON.stringify(record.mismatch).slice(0, 2000)}`)
    if (record.log) console.log(`  log tail ${String(record.log).slice(-1200).replace(/\n/g, "\n  ")}`)
  }
  if (out) appendFileSync(out, `${JSON.stringify(record)}\n`)
}

export function finish(summary, out) {
  console.log(`SUMMARY ${JSON.stringify(summary)}`)
  if (out) appendFileSync(out, `${JSON.stringify({ summary })}\n`)
  process.exit(summary.failed === 0 ? 0 : 1)
}

