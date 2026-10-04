// Unified sandbox refs through the native binding: parse, narrow, the typed
// ambiguity error, and a lookup against an embedded runtime with no Fleet
// credentials (temp state; nothing reaches a cloud or the host).
import assert from "node:assert/strict"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

import * as cua from "../dist/index.js"

test("refs parse, round-trip and keep the legacy spellings", () => {
  for (const ref of ["local:box", "cloud:box", "direct:10.0.0.5:3211", "relay:0123abcd4567ef89"]) {
    assert.equal(cua.parseSandboxRef(ref).id, ref)
  }
  const legacy = cua.parseSandboxRef("space://fleet/ns/box")
  assert.deepEqual([legacy.location, legacy.name, legacy.id], ["cloud", "box", "cloud:box"])
  assert.equal(cua.parseSandboxRef("fleet:ns:box").id, "cloud:box")
  assert.equal(cua.parseSandboxRef("url:h:1").id, "direct:h:1")
  assert.equal(cua.parseSandboxRef("box").location, undefined)
  assert.throws(() => cua.parseSandboxRef("moon:x"), (e) => cua.CuaError.InvalidArgument.instanceOf(e))
  assert.equal(cua.qualifySandboxRef("box", undefined), "box")
  assert.equal(cua.qualifySandboxRef("box", true), "local:box")
  assert.equal(cua.qualifySandboxRef("box", false), "cloud:box")
  assert.throws(() => cua.qualifySandboxRef("cloud:box", true))
})

test("AmbiguousSandbox carries the qualified candidates", () => {
  const e = new cua.CuaError.AmbiguousSandbox('"box" names 2 sandboxes; use one of: local:box, cloud:box')
  assert.ok(cua.CuaError.AmbiguousSandbox.instanceOf(e))
  assert.deepEqual(cua.ambiguousCandidates(e), ["local:box", "cloud:box"])
  assert.deepEqual(cua.ambiguousCandidates(new Error("x")), [])
})

test("every CuaError links to its errors-reference entry", () => {
  const base = "https://cua.ai/docs/cua-sdk/reference/errors#"
  const e = new cua.CuaError.NotFound("x")
  assert.equal(e.docUrl, `${base}notfound`)
  assert.equal(cua.cuaErrorDocUrl(new cua.CuaError.SpacesdNotAvailable("y")), `${base}spacesdnotavailable`)
  assert.equal(cua.cuaErrorDocUrl(new Error("x")), undefined)
})

test("lookups take refs", async () => {
  const dir = mkdtempSync(join(tmpdir(), "cua-ts-refs-"))
  const c = cua.embedded({
    stateDir: join(dir, "sandboxes"),
    spacesHome: join(dir, "cua"),
    fleetPoolHome: join(dir, "pools"),
    fleetFromEnv: false,
    fleetFromSession: false,
  })
  const sbx = c.sandboxes()
  await assert.rejects(sbx.get("nope"), (e) => cua.CuaError.NotFound.instanceOf(e))
  await assert.rejects(sbx.get("cloud:nope"), (e) => cua.CuaError.ProviderNotConfigured.instanceOf(e))
  await assert.rejects(sbx.get("moon:x"), (e) => cua.CuaError.InvalidArgument.instanceOf(e))
})
