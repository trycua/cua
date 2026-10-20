import assert from "node:assert/strict"
import { mkdtempSync, readFileSync, existsSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import { KNOWN_OPS, newNonce, parseScenario, sha256Hex, shellQuote, substitute, writeProfile, xorshiftBytes } from "../dist/core/spec.js"

const specDir = new URL("../../openkoalabot-example-scenario/", import.meta.url)
const specText = readFileSync(new URL("scenario.json", specDir), "utf8")

test("the shared scenario parses and every op is implemented", () => {
  const s = parseScenario(specText)
  assert.equal(s.steps[0].op, "space.open")
  assert.equal(s.steps.at(-1).op, "space.delete")
  for (const step of s.steps) assert.ok(KNOWN_OPS.has(step.op), step.op)
  assert.deepEqual(
    s.steps.filter((x) => x.needs === "model").map((x) => x.op),
    ["routine.schedule", "group.chat"],
  )
  assert.equal(s.model.urlEnv, "OPENKOALABOTS_SCENARIO_MODEL_URL")
})

test("scenario validation names the problem", () => {
  const base = JSON.parse(specText)
  assert.throws(() => parseScenario(JSON.stringify({ ...base, version: 1 })), /unsupported version/)
  assert.throws(() => parseScenario(JSON.stringify({ ...base, model: undefined })), /need a model/)
  assert.throws(() => parseScenario(JSON.stringify({ ...base, steps: [] })), /no steps/)
  assert.throws(() => parseScenario(JSON.stringify({ ...base, steps: [...base.steps, { id: "x", op: "fly" }] })), /unknown op fly/)
  assert.throws(() => parseScenario(JSON.stringify({ ...base, steps: [...base.steps, base.steps[1]] })), /duplicate step id/)
  assert.throws(() => parseScenario(JSON.stringify({ ...base, steps: base.steps.slice(1) })), /first step/)
})

test("placeholders", () => {
  assert.equal(substitute("a {nonce} b {marker} {unknown}", { nonce: "n1", marker: "m" }), "a n1 b m {unknown}")
  assert.match(newNonce(), /^[0-9a-f]{8}$/)
  assert.equal(shellQuote("it's"), `'it'\\''s'`)
})

test("the xorshift fixture matches the spec's sha256", () => {
  const file = parseScenario(specText).steps.find((s) => s.op === "file.send")
  const bytes = xorshiftBytes(file.generate.bytes, BigInt(file.generate.seed))
  assert.equal(bytes.length, 1048576)
  assert.equal(sha256Hex(bytes), file.sha256)
  assert.deepEqual([...xorshiftBytes(4, 1n)], [...xorshiftBytes(4, 1n)], "deterministic")
})

test("the generated profile lands under every root with the marker", () => {
  const fixture = JSON.parse(readFileSync(new URL("fixtures/firefox-profile.json", specDir), "utf8"))
  const home = mkdtempSync(join(tmpdir(), "ogb-profile-"))
  const files = writeProfile(home, fixture, "openkoalabots-cafe")
  assert.equal(files.length, fixture.roots.length * Object.keys(fixture.files).length)
  for (const root of fixture.roots) {
    const prefs = join(home, root, "Profiles/openkoalabots.default-release/prefs.js")
    assert.ok(existsSync(prefs))
    assert.match(readFileSync(prefs, "utf8"), /openkoalabots-cafe/)
  }
})
