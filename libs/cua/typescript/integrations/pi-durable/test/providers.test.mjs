// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

import assert from "node:assert/strict"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import { checkpointLabel, DirectoryCheckpointStore, SandboxSnapshots } from "../dist/index.js"

/** The cua SDK's Sandboxes, as far as checkpoints go, in memory. */
class FakeSandboxes {
  constructor() {
    this.sandboxes = new Map([["space", "s0"]])
    this.saved = new Map()
    this.calls = []
    this.failCheckpointAfterSaving = false
  }
  async get(name) {
    if (!this.sandboxes.has(name)) throw Object.assign(new Error(`sandbox ${name} not found`), { tag: "NotFound" })
    return { name }
  }
  async checkpoint(name, checkpoint) {
    this.calls.push(["checkpoint", name, checkpoint])
    this.saved.set(checkpoint, this.sandboxes.get(name))
    if (this.failCheckpointAfterSaving) throw new Error("connection lost")
    return { name: checkpoint }
  }
  async checkpoints() {
    return [...this.saved.keys()].map((name) => ({ name }))
  }
  async fork(source, name) {
    this.calls.push(["fork", source, name])
    if (this.sandboxes.has(name)) throw new Error(`${name} exists`)
    this.sandboxes.set(name, this.saved.get(source))
    return { name }
  }
  async delete_(name) {
    this.calls.push(["delete", name])
    this.sandboxes.delete(name)
  }
  async deleteCheckpoint(name) {
    this.saved.delete(name)
  }
}

test("machine checkpoints are idempotent by label and survive a lost reply", async () => {
  const sandboxes = new FakeSandboxes()
  const snapshots = new SandboxSnapshots({ sandboxes })
  const id = await snapshots.checkpoint("space", "Space-Call 1")
  assert.equal(id, checkpointLabel("Space-Call 1"))
  assert.equal(await snapshots.checkpoint("space", "Space-Call 1"), id)
  assert.equal(sandboxes.calls.filter(([op]) => op === "checkpoint").length, 1)
  sandboxes.failCheckpointAfterSaving = true
  assert.equal(await snapshots.checkpoint("space", "other"), "other")
})

test("a machine restore replaces the Space with a fork of the checkpoint, and rebuilds a lost one", async () => {
  const sandboxes = new FakeSandboxes()
  const snapshots = new SandboxSnapshots({ sandboxes })
  const id = await snapshots.checkpoint("space", "c1")
  sandboxes.sandboxes.set("space", "s1")
  await snapshots.restore("space", id)
  assert.equal(sandboxes.sandboxes.get("space"), "s0")
  assert.deepEqual(sandboxes.calls.slice(-2), [["delete", "space"], ["fork", id, "space"]])
  // The Space is gone (its machine was lost): fork(id, name, name) builds it again.
  sandboxes.sandboxes.delete("space")
  await snapshots.fork(id, "space", "space")
  assert.equal(sandboxes.sandboxes.get("space"), "s0")
  // A fork to a new name; done again it keeps the existing one.
  await snapshots.fork(id, "space", "child")
  await snapshots.fork(id, "space", "child")
  assert.equal(sandboxes.calls.filter(([op, , name]) => op === "fork" && name === "child").length, 1)
  await assert.rejects(snapshots.restore("space", "missing"), /no checkpoint missing/)
})

test("a directory checkpoint store writes atomically and reads back", async () => {
  const dir = mkdtempSync(join(tmpdir(), "pi-store-"))
  try {
    const store = new DirectoryCheckpointStore(join(dir, "nested"))
    assert.equal(await store.has("c1"), false)
    assert.equal(await store.get("c1"), undefined)
    await store.put("c1", new Uint8Array([1, 2, 3]))
    assert.equal(await store.has("c1"), true)
    assert.deepEqual([...(await store.get("c1"))], [1, 2, 3])
    await store.remove("c1")
    assert.equal(await store.has("c1"), false)
    await assert.rejects(store.put("../escape", new Uint8Array()), /invalid checkpoint id/)
  } finally {
    rmSync(dir, { recursive: true, force: true })
  }
})
