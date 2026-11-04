// Sidecars, private registry secrets and image builds on SandboxCreateOptions
// (hermetic: objects only, nothing is created).
import assert from "node:assert/strict"
import { test } from "node:test"

import * as cua from "../dist/index.js"

test("sidecar() and registrySecret build the native records", () => {
  const db = cua.sidecar("redis:7-alpine", { ports: [6379], env: { A: "1" } })
  assert.equal(db.image, "redis:7-alpine")
  assert.deepEqual(db.ports, [6379])
  assert.equal(db.env.get("A"), "1")
  assert.equal(db.name, undefined, "named from the image by the SDK")

  const basic = cua.registrySecret.basic("me", "tok", "ghcr.io")
  assert.equal(basic.tag, cua.RegistrySecret_Tags.Basic)
  assert.equal(basic.inner.registry, "ghcr.io")
  assert.equal(cua.registrySecret.fromEnv().inner.usernameVar, "CUA_REGISTRY_USERNAME")
  assert.equal(cua.registrySecret.awsEcr().tag, cua.RegistrySecret_Tags.AwsEcr)

  const opts = cua.SandboxCreateOptions.create({
    on: "cloud",
    image: "python:3.12-slim",
    sidecars: [db],
    registrySecret: basic,
    build: cua.ImageBuild.create({
      layers: [new cua.ImageLayer.PipInstall({ packages: ["mcp"] })],
    }),
    services: new Map([["db", 6379]]),
  })
  assert.equal(opts.sidecars.length, 1)
  assert.equal(opts.build.layers.length, 1)
  assert.deepEqual(cua.SandboxCreateOptions.create({ on: "local" }).sidecars, [])
})

test("headless on creates and pools, and a pool's own command", () => {
  assert.equal(cua.SandboxCreateOptions.create({ on: "local" }).headless, false)
  assert.equal(cua.SandboxCreateOptions.create({ on: "local", headless: true }).headless, true)
  const pool = cua.PoolCreateOptions.create({ command: ["sleep", "1"], headless: true })
  assert.deepEqual(pool.command, ["sleep", "1"])
  assert.equal(pool.headless, true)
  assert.equal(cua.PoolCreateOptions.create({}).command, undefined)
  assert.equal(cua.PoolCreateOptions.create({}).headless, false)
  // Spaces have the same switch; the rollout profile turns it on.
  assert.equal(cua.SpaceCreateOptions.create({ headless: true }).headless, true)
  assert.equal(cua.SpaceCreateOptions.create({}).headless, undefined)
})

test("spacesd on creates: any OCI image is a full Space unless it opts out", () => {
  assert.equal(cua.SandboxCreateOptions.create({ on: "local" }).spacesd, undefined)
  assert.equal(cua.SandboxCreateOptions.create({ on: "local", spacesd: false }).spacesd, false)
  assert.equal(cua.SpaceCreateOptions.create({}).spacesd, undefined)
  assert.equal(cua.SpaceCreateOptions.create({ spacesd: false }).spacesd, false)
})

test("VM density: the profile create option and the HostCapacity error", () => {
  const o = cua.SandboxCreateOptions.create({ on: "local", image: "x", profile: "rollout" })
  assert.equal(o.profile, "rollout")
  assert.equal(cua.SandboxCreateOptions.create({ on: "local" }).profile, undefined)
  assert.equal(cua.SpaceCreateOptions.create({ profile: "rollout" }).profile, "rollout")
  const e = new cua.CuaError.HostCapacity("at its VM CPU limit")
  assert.ok(cua.CuaError.HostCapacity.instanceOf(e))
  assert.match(String(e.message ?? e), /VM CPU limit/)
})

test("named volumes are a create option and a record", () => {
  const m = cua.VolumeMount.create({ name: "shared", path: "/mnt/shared" })
  assert.equal(m.readOnly, false)
  assert.equal(m.subpath, "")
  const ro = cua.VolumeMount.create({ name: "data", path: "/data", readOnly: true, subpath: "in" })
  const o = cua.SpaceCreateOptions.create({ volumes: [m, ro] })
  assert.deepEqual(o.volumes.map((v) => v.path), ["/mnt/shared", "/data"])
  assert.equal(o.volumes[1].readOnly, true)
  assert.equal(o.volumes[1].subpath, "in")
  assert.deepEqual(cua.SpaceCreateOptions.create({}).volumes, [])
})

test("egress: the policy record and where it is enforced", () => {
  assert.deepEqual(cua.EgressPolicy.create({}), { allow: [], deny: [] })
  const p = cua.EgressPolicy.create({ allow: ["1.1.1.1"], deny: ["all"] })
  assert.deepEqual([p.allow, p.deny], [["1.1.1.1"], ["all"]])
  assert.equal(cua.SpaceCreateOptions.create({ egress: p }).egress, p)
  // The one place a policy is enforced: by the host, outside the guest.
  assert.deepEqual(Object.keys(cua.EgressEnforcement).filter((k) => isNaN(Number(k))), ["Host"])
})
