import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"

test("lookup diagnostics distinguish qualification failures from workload failures", async () => {
  const contract = await readFile(new URL("../docs/fleet-attribution-phase1.md", import.meta.url), "utf8")
  assert.match(contract, /shared 250 ms deadline/i)
  assert.match(contract, /qualification_lookup_stage/)
  assert.match(contract, /qualification_error_class/)
  assert.match(contract, /Both properties must be present together, use allowlisted values/i)
  assert.match(contract, /neither a proven failed workload nor a proven qualifying success/i)
  assert.match(contract, /Historical rejections without the new fields cannot establish the underlying failure cause/i)
})

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

test("public contract matches backend identity classification and delivery exclusions", async () => {
  const contract = await readFile(new URL("../docs/fleet-attribution-phase1.md", import.meta.url), "utf8")
  assert.match(contract, /verified `@trycua\.com` email evidence or trusted admin-owner membership classifies the identity as internal/i)
  assert.match(contract, /without verified company-domain evidence, unresolved admin membership leaves the identity unknown/i)
  assert.match(contract, /unauthenticated identities and identities without a stable user ID also remain unknown/i)
  assert.match(contract, /resolved non-admin identities without verified company-domain evidence are external, including those with missing or unverified email evidence/i)
  assert.match(contract, /internal and unknown identities are discarded without emitting `fleet_attribution_bound`/i)
  assert.match(contract, /configured subject exclusions suppress analytics delivery/i)
  assert.match(contract, /only as set-once first-touch person properties under the keyed-HMAC pseudonym/i)
  assert.match(contract, /never accepts a URL, referrer, email, raw identity, arbitrary property, or provider payload/i)
  assert.doesNotMatch(contract, /missing or unverified email evidence is treated as external/i)
})
