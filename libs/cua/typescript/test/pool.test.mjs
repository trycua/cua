// Local pools: the PoolCreateOptions record, and that pools need the cua
// daemon and this machine.
import assert from "node:assert/strict"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

import * as cua from "../dist/index.js"

test("poolCreateOptions builds the native record; pools need the daemon and this machine", async () => {
  const o = cua.poolCreateOptions({ name: "agents", min: 2, max: 8, idleSeconds: 600, env: { A: "1" } })
  assert.equal(o.name, "agents")
  assert.equal(o.min, 2)
  assert.equal(o.max, 8)
  assert.equal(o.idleMs, 600_000n)
  assert.equal(o.env.get("A"), "1")
  assert.equal(cua.poolCreateOptions().min, 1, "min defaults to 1")
  const c = cua.embedded({ stateDir: join(mkdtempSync(join(tmpdir(), "cua-ts-lpool-")), "sandboxes") })
  await assert.rejects(
    c.sandboxes().createPool("python:3.12-slim", cua.poolCreateOptions({ on: "cloud" })),
    (e) => cua.CuaError.Unsupported.instanceOf(e) && /on="local"/.test(e.message),
  )
  await assert.rejects(
    c.sandboxes().createPool("python:3.12-slim", cua.poolCreateOptions()),
    (e) => cua.CuaError.Unsupported.instanceOf(e) && /cua daemon/.test(e.message),
  )
})
