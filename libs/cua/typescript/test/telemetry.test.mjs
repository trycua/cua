// The Node binding's telemetry surface. Offline: telemetry is forced off
// and the network guard is on before the binding loads.
import assert from "node:assert/strict"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

process.env.CUA_TELEMETRY = "0"
process.env.CUA_TELEMETRY_FORBID_NETWORK = "1"
// DO_NOT_TRACK outranks CUA_TELEMETRY; CI sets it, so drop it to test the
// CUA_TELEMETRY source asserted below (telemetry stays off either way).
delete process.env.DO_NOT_TRACK
process.env.CUA_HOME = mkdtempSync(join(tmpdir(), "cua-telemetry-ts-"))

const cua = await import("../dist/index.js")

test("status is off in tests and names the TypeScript surface", () => {
  const s = cua.telemetryStatus()
  assert.equal(s.enabled, false)
  assert.equal(s.sourceKind, "env")
  assert.equal(s.product, "sdk_typescript")
  const envelope = JSON.parse(s.envelopeJson)
  assert.equal(envelope.$geoip_disable, true)
  assert.equal(envelope.$process_person_profile, false)
})

test("app events outside the vocabulary are dropped, and nothing is queued while off", () => {
  assert.equal(cua.telemetryRecordFeature("/Users/alice/secret"), false)
  assert.equal(cua.telemetryRecordFeature("teleport_drop"), false)
  assert.deepEqual(JSON.parse(cua.telemetryShowLast(10)), [])
})

test("data sharing is not exposed", () => {
  // Opt-in data sharing is parked: no consent API in any binding.
  assert.equal(cua.dataSharingStatus, undefined)
  assert.equal(cua.dataSharingGrant, undefined)
  assert.equal("dataSharingGranted" in cua.telemetryStatus(), false)
})
