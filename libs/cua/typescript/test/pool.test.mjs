// The shared sandbox model (SandboxSpec + PoolOptions). The one pool writer
// (apply, the named-pool mismatch check, template reconcile, Terraform
// export) needed a Fleet; Cua Cloud has ended, so `fleet()` fails and that
// test is skipped until it is removed with the Fleet provider.
import assert from "node:assert/strict"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { after, before, test } from "node:test"

import * as cua from "../dist/index.js"
import { startFixtures } from "./fixtures.mjs"

let fx
before(async () => {
  fx = await startFixtures()
})
after(async () => {
  await fx?.stop()
})

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

test("fleet() fails with the cloud-ended error, so there is no pool writer", () => {
  const c = cua.embedded({
    stateDir: join(mkdtempSync(join(tmpdir(), "cua-ts-pool-")), "sandboxes"),
    fleetFromEnv: false,
  })
  assert.throws(() => c.fleet(), (e) => {
    assert.ok(cua.CuaError.Unsupported.instanceOf(e), String(e))
    assert.match(e.message, /Cua Cloud has ended/)
    return true
  })
})

test("Pool.apply, checkPoolSpec, applyPoolTemplate and exportPool on the fake Fleet", { skip: "Cua Cloud has ended (CUA-1854); removed with the Fleet provider" }, async () => {
  const c = cua.embedded({
    stateDir: join(mkdtempSync(join(tmpdir(), "cua-ts-pool-")), "sandboxes"),
    fleetFromEnv: false,
    fleet: cua.FleetSettings.create({ baseUrl: fx.fleet_base_url, token: fx.fleet_token }),
  })
  const fleet = c.fleet()
  const name = "cua-e2e-ts-apply"
  // Digest-pinned and an explicit runtime: nothing reads a registry.
  const spec = cua.sandboxSpec("ghcr.io/trycua/cua-e2e-ts@sha256:0123", {
    command: ["python", "-m", "srv"],
    services: { mcp: 8765 },
    cpu: 2,
    memoryMb: 2048,
  })
  const pool = await cua.Pool.apply(fleet, name, spec, cua.poolOptions({ runtime: "gvisor", warm: true, maxPoolSize: 3, idleTtlSeconds: 3600 }))
  assert.equal(pool.name, name)
  assert.equal(pool.replicas, 1, "warm is an explicit floor of one")

  await fleet.checkPoolSpec(name, spec)
  await fleet.checkPoolSpec(name, cua.sandboxSpec("", { command: ["python", "-m", "srv"] }))
  const other = cua.sandboxSpec("", { command: ["node", "srv.js"], cpu: 4 })
  await assert.rejects(fleet.checkPoolSpec(name, other), (e) => {
    assert.ok(cua.CuaError.PoolSpecMismatch.instanceOf(e), String(e))
    assert.match(e.message, /command: pool has \["python", "-m", "srv"\], requested \["node", "srv.js"\]/)
    assert.match(e.message, /cpu: pool has 2, requested 4/)
    return true
  })

  await fleet.applyPoolTemplate(name, other)
  await fleet.checkPoolSpec(name, other)

  const exported = await fleet.exportPool(name)
  assert.equal(exported.runtime, "gvisor")
  assert.deepEqual(exported.spec.command, ["node", "srv.js"])
  assert.equal(exported.spec.memoryMb, 2048, "untouched by applyPoolTemplate")
  assert.equal(exported.options.idleTtlSeconds, 3600)
  assert.match(exported.terraform, /resource "fleets_pool" "cua_e2e_ts_apply" \{/)
  assert.match(exported.terraform, /\n  command = \["node", "srv.js"\]\n/)
  assert.match(exported.terraform, /min_pool_size = 1/)

  await fleet.deletePool(name)
})
