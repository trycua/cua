// Space labels through the Node binding: the options and SpaceInfo types,
// and the limits checked before anything is created. Creating a labelled
// Space needs a VM (see crates/cua-spaces/tests/generic.rs and the live
// runs).
import assert from "node:assert/strict"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { after, before, test } from "node:test"

import * as cua from "../dist/index.js"
import { binary, startFixtures } from "./fixtures.mjs"

let fx
before(async () => {
  fx = await startFixtures()
})
after(async () => {
  await fx?.stop()
})

test("labels: types and limits", { skip: !binary("CUA_TEST_FIXTURES", "cua-test-fixtures") }, async () => {
  const dir = mkdtempSync(join(tmpdir(), "cua-ts-labels-"))
  const c = cua.embedded({ stateDir: join(dir, "sbx"), spacesHome: join(dir, "cua") })
  const spaces = c.spaces()
  const info = await spaces.add(fx.spaces_url, fx.spaces_token, "ts-labels")
  assert.equal(info.labels.size, 0)

  const create = (labels, extra = {}) =>
    spaces.create(cua.SpaceCreateOptions.create({ on: "local", labels: new Map(Object.entries(labels)), ...extra }))
  const many = Object.fromEntries(Array.from({ length: 65 }, (_, i) => [`k${i}`, "v"]))
  for (const labels of [{ "": "v" }, { k: "" }, { ["k".repeat(64)]: "v" }, { k: "v".repeat(257) }, many]) {
    await assert.rejects(create(labels), (e) => e instanceof cua.CuaError.InvalidArgument)
  }
  for (const extra of [{ durable: true }, { reuse: true }]) {
    await assert.rejects(create({ a: "b" }, extra), (e) => e instanceof cua.CuaError.InvalidArgument && /labels/.test(e.message))
  }
  await spaces.delete_(info.id)
})
