// The checkpoint and fork API of the TypeScript binding (hermetic: methods
// and records only, nothing is created), so it cannot drift from the Rust
// SDK it is generated from.
import assert from "node:assert/strict"
import { test } from "node:test"

import * as cua from "../dist/index.js"

/** A method's parameters as written (async options last). */
function params(fn) {
  const src = fn.toString()
  return src
    .slice(src.indexOf("(") + 1, src.indexOf(")"))
    .split(",")
    .map((p) => p.trim())
    .filter(Boolean)
}

test("Sandboxes.checkpoint takes pin, defaulting to false", () => {
  assert.deepEqual(params(cua.Sandboxes.prototype.checkpoint), [
    "name",
    "checkpoint",
    "pin = false",
    "asyncOpts_",
  ])
})

test("Sandboxes pins and prunes checkpoints", () => {
  assert.deepEqual(params(cua.Sandboxes.prototype.pinCheckpoint), ["name", "pinned", "asyncOpts_"])
  assert.deepEqual(params(cua.Sandboxes.prototype.pruneCheckpoints), ["asyncOpts_"])
})

test("a Space checkpoints and forks; Spaces fork from a checkpoint", () => {
  assert.deepEqual(params(cua.Space.prototype.checkpoint), ["checkpoint", "pin = false", "asyncOpts_"])
  assert.deepEqual(params(cua.Space.prototype.fork), ["name", "asyncOpts_"])
  assert.deepEqual(params(cua.Spaces.prototype.fork), ["source", "name", "asyncOpts_"])
})

test("CheckpointSummary says pinned and sharedMemory", () => {
  const c = cua.CheckpointSummary.create({
    name: "world",
    source: "bake",
    runtimeType: "qemu",
    createdAt: "2026-10-19T00:00:00Z",
    image: "ghcr.io/trycua/linux:24.04",
    pinned: true,
    sharedMemory: true,
  })
  assert.equal(c.pinned, true)
  assert.equal(c.sharedMemory, true)
})

test("create options take forkable", () => {
  assert.equal(cua.SandboxCreateOptions.create({ image: "linux" }).forkable, undefined)
  assert.equal(cua.SandboxCreateOptions.create({ image: "linux", forkable: true }).forkable, true)
  assert.equal(cua.SpaceCreateOptions.create({}).forkable, undefined)
  assert.equal(cua.SpaceCreateOptions.create({ forkable: true }).forkable, true)
})
