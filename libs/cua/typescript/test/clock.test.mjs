// When a Space was created and when it ends (SpaceInfo.createdAtUnix /
// expiresAtUnix / onExpire), read back through the Node binding against the
// fixture's in-process cua-spacesd core. No VMs.
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

test("created and expires are listed", { skip: !binary("CUA_TEST_FIXTURES", "cua-test-fixtures") }, async () => {
  const dir = mkdtempSync(join(tmpdir(), "cua-ts-clock-"))
  const c = cua.embedded({ stateDir: join(dir, "sbx"), spacesHome: join(dir, "cua") })
  const spaces = c.spaces()
  const t0 = Math.floor(Date.now() / 1000)
  const info = await spaces.add(fx.spaces_url, fx.spaces_token, "ts-clock")
  assert.ok(info.createdAtUnix >= BigInt(t0 - 2) && info.createdAtUnix <= BigInt(t0 + 10), String(info.createdAtUnix))
  assert.equal(info.expiresAtUnix, 0n)
  assert.equal(info.onExpire, "")

  const at = await spaces.setLifetime(info.id, 60n, undefined)
  let row = (await spaces.list())[0]
  assert.equal(row.expiresAtUnix, at)
  assert.equal(row.onExpire, "delete")
  assert.equal(row.createdAtUnix, info.createdAtUnix)

  const later = await spaces.setLifetime(info.id, 600n, "suspend")
  row = (await spaces.list())[0]
  assert.equal(row.expiresAtUnix, later)
  assert.equal(row.onExpire, "suspend")
  await assert.rejects(spaces.setLifetime(info.id, 1n, "explode"))
  await spaces.delete_(info.id)
})
