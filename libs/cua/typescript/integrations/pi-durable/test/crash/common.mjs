// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// Shared pieces of the crash campaign's modes: worker processes with marker
// parsing, the committed-log and workspace readers, the seeded random source
// and the summary conventions.

import { spawn } from "node:child_process"
import { BACKGROUND_CONTEXT as context } from "@earendil-works/chord/context"
import { createModels } from "@earendil-works/pi-ai"
import { createRegistry, Harness } from "@earendil-works/pi-durable"
import { openNodeSqliteStorage } from "@earendil-works/pi-durable/storage/sqlite/node"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

export const here = dirname(fileURLToPath(import.meta.url))
export const AGENT = join(here, "agent.mjs")

/** Deterministic random source for a seed. */
export function seeded(seed) {
  let state = (seed * 2654435761) % 2147483648
  return () => {
    state = (state * 1103515245 + 12345) % 2147483648
    return state / 2147483648
  }
}

export const pick = (random, list) => list[Math.floor(random() * list.length)]

/** `MARK <point>` or `MARK <point> @<ms>` -> `{ point, at }`. */
export function parseMark(line) {
  const m = /^MARK (\S+)(?: @(\d+))?$/.exec(line)
  return m ? { point: m[1], at: m[2] ? Number(m[2]) : undefined } : undefined
}

/**
 * Starts a worker process and calls `onLine(line, control)` for every stdout line.
 * Returns `{ child, exited }`; `exited` resolves with the exit status and what
 * the worker printed (`DONE`, `STALE`, `RECOVERY`, stderr tail in `log`).
 */
export function spawnWorker(env, { onLine, log = [] } = {}) {
  // A runaway worker must not take the machine down: bound its heap.
  const child = spawn(process.execPath, [`--max-old-space-size=${process.env.CRASH_NODE_HEAP_MB ?? 768}`, AGENT], { env, stdio: ["ignore", "pipe", "pipe"] })
  const state = { done: undefined, stale: undefined, recoveries: [], marks: [], lines: [], firstMarkAt: undefined }
  let buffer = ""
  child.stdout.on("data", (chunk) => {
    buffer += chunk.toString("utf8")
    let newline
    while ((newline = buffer.indexOf("\n")) !== -1) {
      const line = buffer.slice(0, newline)
      buffer = buffer.slice(newline + 1)
      state.lines.push(line)
      if (state.lines.length > 60) state.lines.shift()
      if (line.startsWith("DONE")) state.done = line
      else if (line.startsWith("STALE ")) state.stale = line
      else if (line.startsWith("RECOVERY ")) state.recoveries.push(JSON.parse(line.slice(9)))
      else {
        const mark = parseMark(line)
        if (mark) {
          state.firstMarkAt ??= Date.now()
          state.marks.push(mark.point)
          onLine?.(mark, child)
        }
      }
    }
  })
  child.stderr.on("data", (chunk) => log.push(chunk.toString("utf8")))
  const exited = new Promise((resolve) => {
    child.on("exit", (code, signal) => resolve({ code, signal, ...state, tail: state.lines.join("\n") }))
  })
  return { child, exited, state }
}

/** Runs a worker, SIGKILLing it `fault.delay` ms after the `fault.occurrence`-th `fault.point` mark. */
export async function startWorker(env, fault, log) {
  let seen = 0
  let killed = false
  let killedAt
  const { exited } = spawnWorker(env, {
    log,
    onLine(mark, child) {
      if (fault && mark.point === fault.point && ++seen === fault.occurrence && !killed) {
        setTimeout(() => {
          if (child.exitCode === null) {
            killed = true
            killedAt = Date.now()
            child.kill("SIGKILL")
          }
        }, fault.delay)
      }
    },
  })
  const result = await exited
  return { ...result, killed, killedAt }
}

/** Pi's committed transcript in the SQLite log at `db`. */
export async function committedMessages(db) {
  const storage = await openNodeSqliteStorage(db)
  const harness = await Harness.open(storage, { models: createModels(), registry: createRegistry() }, context)
  try {
    const root = await harness.root(context)
    return (await root.context(context)).messages
  } finally {
    await harness.close(context)
  }
}

/** An error's tag, message and stack (SDK errors keep their text in `message`). */
export function describe(error) {
  if (!(error instanceof Error)) return String(error)
  const tag = error.tag ? `${error.tag}: ` : ""
  return `${tag}${error.message}\n${error.stack ?? ""}`
}

export function percentile(values, p) {
  if (values.length === 0) return undefined
  const sorted = [...values].sort((a, b) => a - b)
  return sorted[Math.min(sorted.length - 1, Math.floor((p / 100) * sorted.length))]
}

/** The summary every mode prints as `SUMMARY <json>`; modes add their own numbers. */
export function newSummary(extra = {}) {
  return { runs: 0, ok: 0, failed: 0, kills: 0, faults: {}, recoveries: {}, interrupted: 0, ...extra }
}

export function tally(summary, record) {
  for (const r of record.recoveries ?? []) summary.recoveries[r.kind] = (summary.recoveries[r.kind] ?? 0) + 1
  summary.runs++
  summary.interrupted += record.interrupted ?? 0
  if (record.ok) summary.ok++
  else summary.failed++
}

/** Sorted `{count,p50,p95,max}` of resume latencies (ms) for a summary. */
export function latencyStats(values) {
  return values.length === 0
    ? { count: 0 }
    : { count: values.length, p50: percentile(values, 50), p95: percentile(values, 95), max: Math.max(...values) }
}
