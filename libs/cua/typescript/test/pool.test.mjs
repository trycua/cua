// The shared sandbox model (SandboxSpec + PoolOptions) and the pool writer
// (Cua Cloud, closed: it says so).
import assert from "node:assert/strict"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

import * as cua from "../dist/index.js"

test("sandboxSpec / poolOptions build the native records", () => {
  const spec = cua.sandboxSpec("python:3.12-slim", {
    command: ["python", "-m", "srv"],
    env: { A: "1" },
    services: { mcp: 8765 },
    readiness: cua.http("mcp", "/health"),
    claimSecrets: true,
  })
  assert.equal(spec.image, "python:3.12-slim")
  assert.equal(spec.env.get("A"), "1")
  assert.equal(spec.services.get("mcp"), 8765)
  assert.equal(spec.readiness.httpPath, "/health")
  assert.equal(spec.claimSecrets, true)
  assert.deepEqual(spec.sidecars, [])
  const opts = cua.poolOptions({ warm: true, idleTtlSeconds: 3600, ttlPolicy: "Cascade" })
  assert.equal(opts.warm, true)
  assert.equal(opts.maxPoolSize, undefined)
  assert.equal(cua.CloudOptions.create({ pool: "p", apply: true }).apply, true)
  assert.equal(cua.CloudOptions.create({}).apply, false)
  assert.equal(cua.fleetGenerateClaimToken().length, 64)
})

test("Pool.apply says Cua Cloud has closed", async () => {
  const c = cua.embedded({
    stateDir: join(mkdtempSync(join(tmpdir(), "cua-ts-pool-")), "sandboxes"),
    fleetFromEnv: false,
    fleet: cua.FleetSettings.create({ baseUrl: "http://127.0.0.1:9", token: "t" }),
  })
  const spec = cua.sandboxSpec("ghcr.io/trycua/cua-e2e-ts@sha256:0123", { services: { mcp: 8765 } })
  await assert.rejects(
    cua.Pool.apply(c.fleet(), "cua-e2e-ts-apply", spec, cua.poolOptions({ runtime: "gvisor" })),
    (e) => cua.CuaError.Fleet.instanceOf(e) && /Cua Cloud has closed/.test(e.message),
  )
})
