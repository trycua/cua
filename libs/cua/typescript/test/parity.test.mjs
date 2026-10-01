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
