// Sandboxes.bake: the options record and the checks that run before any VM
// starts (the bake itself is the docker tier's integration test,
// libs/images/linux/tests/bake-world.sh).
import assert from "node:assert/strict"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

import * as cua from "../dist/index.js"

test("BakeOptions defaults", () => {
  const o = cua.BakeOptions.create({ projectDir: "/worlds/w", checkpoint: "w1" })
  assert.equal(o.image, undefined)
  assert.deepEqual(o.composeFiles, [])
  assert.deepEqual(o.imageTarballs, [])
  assert.deepEqual(o.health, [])
  assert.deepEqual(o.run, [])
  assert.equal(o.healthTimeoutS, 600)
  assert.equal(o.verifyFork, false)
  assert.equal(o.keep, false)
})

test("bake checks its inputs before starting a VM", async () => {
  const dir = mkdtempSync(join(tmpdir(), "cua-ts-bake-"))
  const c = cua.embedded({ stateDir: join(dir, "sbx") })
  const sbx = c.sandboxes()
  await assert.rejects(
    sbx.bake(cua.BakeOptions.create({ projectDir: join(dir, "missing"), checkpoint: "w1" })),
    (e) => cua.CuaError.NotFound.instanceOf(e),
  )
  await assert.rejects(
    sbx.bake(cua.BakeOptions.create({ projectDir: dir, checkpoint: " " })),
    (e) => cua.CuaError.InvalidArgument.instanceOf(e),
  )
})
