// Batched forks through the Node binding (`Sandboxes.forkMany`,
// `forkGroups`, `publishCheckpoint`). Hermetic: an embedded runtime with a
// private home and checkpoints that do not exist, so every fork fails on
// its own and no VM starts.
import assert from "node:assert/strict"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

import * as cua from "../dist/index.js"

test("fork batch records carry their defaults", () => {
  const o = cua.ForkManyOptions.create({})
  assert.equal(o.on, undefined)
  assert.equal(o.groupSize, 0)
  assert.equal(o.noRetry, false)
  const g = cua.ForkGroup.create({ source: "world", count: 8 })
  assert.equal(g.namePrefix, "")
  const f = cua.SnapshotFallbackOptions.create({ dir: "~/Cua Volume/snapshots" })
  assert.equal(f.s3Bucket, "")
})

test("forkMany streams each fork, then the summary", async () => {
  const home = mkdtempSync(join(tmpdir(), "cua-ts-fork-batch-"))
  const c = cua.embedded({ stateDir: join(home, "sbx") })
  const batch = await c
    .sandboxes()
    .forkMany(
      "no-such-checkpoint",
      3,
      cua.ForkManyOptions.create({ on: "local", groupSize: 2, namePrefix: "ts" }),
    )
  const events = []
  for (;;) {
    const e = await batch.next()
    if (e === undefined) break
    events.push(e)
  }
  const placements = events.filter((e) => e.tag === cua.ForkBatchEvent_Tags.Placement)
  const items = events
    .filter((e) => e.tag === cua.ForkBatchEvent_Tags.Item)
    .map((e) => e.inner.item)
    .sort((a, b) => a.index - b.index)
  assert.equal(placements.length, 2)
  assert.deepEqual(
    items.map((i) => i.name),
    ["ts-g0-0", "ts-g0-1", "ts-g1-0"],
  )
  assert.ok(items.every((i) => i.errorCode === "sync_failed" && i.space === ""))
  const last = events.at(-1)
  assert.equal(last.tag, cua.ForkBatchEvent_Tags.Done)
  assert.equal(last.inner.summary.failed, 3)

  const all = await (
    await c.sandboxes().forkGroups([cua.ForkGroup.create({ source: "w-a", count: 2, namePrefix: "a" })], undefined)
  ).collect()
  assert.deepEqual(
    all.items.map((i) => i.name),
    ["a-0", "a-1"],
  )
  await assert.rejects(
    c.sandboxes().publishCheckpoint(
      "no-such-checkpoint",
      cua.SnapshotFallbackOptions.create({ dir: join(home, "vol") }),
    ),
  )
})
