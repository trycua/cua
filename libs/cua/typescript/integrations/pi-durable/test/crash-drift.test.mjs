// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// Drift guard of the crash campaign: its fault-point list, the markers the
// durable layer and the worker really emit, its modes, the npm scripts that
// run them, and the guide that documents them must agree.

import assert from "node:assert/strict"
import { existsSync, readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { test } from "node:test"
import { fileURLToPath } from "node:url"
import { FAULTS_BY_MODE, MODES, PROGRESS_MARKS, RECOVERY_POINTS } from "./crash/faults.mjs"

const here = dirname(fileURLToPath(import.meta.url))
const pkgDir = join(here, "..")
const read = (...p) => readFileSync(join(pkgDir, ...p), "utf8")
const GUIDE = join(pkgDir, "../../../../../docs/content/docs/spaces/guides/pi-durable.mdx")

const emitted = (source, fn) => new Set([...source.matchAll(new RegExp(`\\b${fn}\\("([a-z-]+)"\\)`, "g"))].map((m) => m[1]))
const durable = emitted(read("src/durable.ts"), "trace")
const worker = emitted(read("test/crash/agent.mjs"), "mark")
const all = new Set([...durable, ...worker])
// ACP mode's faults are kinds of kill acp.mjs injects, not markers (checked by their own test below).
const listed = new Set(Object.entries(FAULTS_BY_MODE).filter(([mode]) => mode !== "acp").flatMap(([, list]) => list.map(([point]) => point)))

test("every fault point is a marker that durable.ts or agent.mjs emits", () => {
  for (const point of listed) assert.ok(all.has(point), `fault point ${point} is not emitted by src/durable.ts (trace) or test/crash/agent.mjs (mark)`)
})

test("every marker the durable layer or the worker emits is a fault point or a declared progress mark", () => {
  for (const point of all) {
    assert.ok(listed.has(point) || PROGRESS_MARKS.includes(point), `marker ${point} is emitted but is neither in test/crash/faults.mjs nor a progress mark`)
  }
  for (const point of PROGRESS_MARKS) assert.ok(all.has(point), `progress mark ${point} is no longer emitted`)
  for (const point of RECOVERY_POINTS) assert.ok(listed.has(point), `recovery point ${point} is not a fault point`)
})

test("the transactional list covers the transaction's steps (intent, executed, diff, outcome, undo)", () => {
  const points = FAULTS_BY_MODE.transactional.map(([p]) => p)
  for (const need of ["started", "executed", "diffed", "outcome", "undo-committed", "undoing", "reverted"]) assert.ok(points.includes(need), need)
})

test("every mode has fault points, an npm script, and a branch in run.mjs", () => {
  const scripts = JSON.parse(read("package.json")).scripts
  const run = read("test/crash/run.mjs")
  for (const mode of MODES) {
    assert.ok(FAULTS_BY_MODE[mode]?.length > 0, `mode ${mode} has no fault points`)
    assert.ok(scripts[`test:crash:${mode}`]?.includes(`CRASH_MODE=${mode}`), `package.json lacks test:crash:${mode}`)
    assert.ok(run.includes(`//   ${mode}`), `run.mjs does not document mode ${mode}`)
  }
  assert.ok(scripts["test:crash:ci"], "package.json lacks test:crash:ci")
  assert.ok(existsSync(join(here, "crash/stale.mjs")) && existsSync(join(here, "crash/redrive.mjs")))
})

test("every ACP fault kind is handled by acp.mjs, and the preload implements the intent points", () => {
  const acp = read("test/crash/acp.mjs")
  const preload = read("test/crash/acp-fault.mjs")
  const runner = readFileSync(join(pkgDir, "../../../crates/cua-agents/runner/acp-runner.mjs"), "utf8")
  assert.ok(!/TEST_FAULT/.test(runner), "the shipped runner must not carry a test fault hook")
  for (const [kind] of FAULTS_BY_MODE.acp) {
    assert.ok(acp.includes(`"${kind}"`), `acp.mjs does not handle the fault kind ${kind}`)
    if (kind.endsWith("_intent")) assert.ok(preload.includes(`"${kind}"`), `acp-fault.mjs does not implement ${kind}`)
  }
})

test("test:crash:ci runs only modes that need no Docker and no lab", () => {
  assert.ok(JSON.parse(read("package.json")).scripts["test:crash:ci"].includes("test/crash/ci.mjs"))
  const ci = read("test/crash/ci.mjs")
  assert.ok(ci.includes('CRASH_BACKEND: "local"'), "the CI subset must use the local backend")
  assert.ok(!/mode: "redrive"/.test(ci), "redrive needs the lab coordinator")
  for (const m of ["transactional", "stale"]) assert.ok(ci.includes(`mode: "${m}"`), `the CI subset lacks ${m}`)
})

test("the guide documents the crash campaign: every mode, its npm script, and the CI subset", { skip: !existsSync(GUIDE) }, () => {
  const guide = readFileSync(GUIDE, "utf8")
  assert.match(guide, /Crash campaign/)
  for (const mode of MODES) {
    assert.ok(guide.includes(`CRASH_MODE=${mode}`) || guide.includes(`test:crash:${mode}`), `the guide does not mention mode ${mode}`)
  }
  assert.ok(guide.includes("test:crash:ci"), "the guide does not mention test:crash:ci")
  assert.ok(guide.includes("CRASH_LAB_BIN"), "the guide does not mention CRASH_LAB_BIN")
})

test("redrive mode reads the lab coordinator from CRASH_LAB_BIN", () => {
  assert.ok(read("test/crash/redrive.mjs").includes("CRASH_LAB_BIN"))
})
