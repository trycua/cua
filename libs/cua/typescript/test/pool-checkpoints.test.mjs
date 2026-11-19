// Checkpoints of Spaces on any of your machines, through the Node binding:
// `spaces().create` with image `checkpoint:<name>` (a fork, started where
// the checkpoint is), checkpoints across machines and
// `CheckpointSummary.host`. Hermetic: an embedded runtime with a private
// home and no other machines, so nothing starts.
import assert from "node:assert/strict"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

import * as cua from "../dist/index.js"

test("a checkpoint summary says where it is", () => {
  const c = cua.CheckpointSummary.create({
    name: "world",
    source: "baker",
    runtimeType: "qemu",
    createdAt: "2026-10-21T00:00:00Z",
    image: "ghcr.io/trycua/linux:24.04",
    pinned: false,
    sharedMemory: true,
    releaseWithForks: false,
    forks: 0,
    host: "kvm-a",
  })
  assert.equal(c.host, "kvm-a")
})

test("a checkpoint image is a fork, not an image", async () => {
  const home = mkdtempSync(join(tmpdir(), "cua-ts-pool-checkpoints-"))
  const c = cua.embedded({
    stateDir: join(home, "sbx"),
    spacesHome: join(home, "home"),
  })
  await assert.rejects(
    c.spaces().create(cua.SpaceCreateOptions.create({ image: "checkpoint:nowhere" })),
    /no machine of yours holds it/,
  )
  await assert.rejects(
    c.spaces().create(cua.SpaceCreateOptions.create({ image: "checkpoint:world", command: ["true"] })),
    /takes no command/,
  )
  assert.deepEqual(await c.sandboxes().checkpoints(), [])
  await assert.rejects(c.sandboxes().deleteCheckpoint("nowhere"))
  await assert.rejects(c.sandboxes().checkpoint("no-such-space", "cp", false))
})
