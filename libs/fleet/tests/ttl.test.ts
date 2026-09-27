import assert from "node:assert/strict"
import test from "node:test"
import { formatTtl } from "../src/fleet/ttl"

test("formats TTL values for fleet tables", () => {
  assert.equal(formatTtl(undefined), "None")
  assert.equal(formatTtl(0), "0s")
  assert.equal(formatTtl(90), "1m 30s")
  assert.equal(formatTtl(90_061), "1d 1h 1m 1s")
})
