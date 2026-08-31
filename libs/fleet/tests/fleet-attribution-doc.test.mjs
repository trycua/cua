import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

test("public contract is channel-neutral and says unknown query keys are ignored", async () => {
  const contract = await readFile(new URL("../docs/fleet-attribution-phase1.md", import.meta.url), "utf8")
  assert.match(contract, /acquisition-channel neutral/i)
  assert.match(contract, /utm_source=x.*utm_medium=organic-social/s)
  assert.match(contract, /utm_source=x.*utm_medium=paid-social/s)
  assert.match(contract, /unknown query keys are ignored/i)
  assert.match(contract, /pixels.*optional downstream adapters/i)
  assert.match(contract, /keyed-HMAC pseudonym/i)
  assert.match(contract, /internal and unknown identities are discarded/i)
  assert.doesNotMatch(contract, /Unknown keys.*rejected/i)
})
