// Egress policies through the Node binding: the types (the policy and
// where it is enforced), the options, the input validation and the
// refusals, all decided before anything runs. Enforcement is the host's
// (nothing here touches a firewall); the rules are covered by the unit
// tests in crates/cua-vmm/src/egress.rs and the opt-in runs in
// crates/cua-daemon/tests/space_egress.rs prove them against a guest root.
import assert from "node:assert/strict"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { after, before, test } from "node:test"

import * as cua from "../dist/index.js"
import { binary, startFixtures } from "./fixtures.mjs"

let fx
before(async () => {
  fx = await startFixtures()
})
after(async () => {
  await fx?.stop()
})

const invalid = (e) => e instanceof cua.CuaError.InvalidArgument

test("egress: types and validation", { skip: !binary("CUA_TEST_FIXTURES", "cua-test-fixtures") }, async () => {
  const dir = mkdtempSync(join(tmpdir(), "cua-ts-egress-"))
  const c = cua.embedded({ stateDir: join(dir, "sbx"), spacesHome: join(dir, "cua") })
  const spaces = c.spaces()
  const info = await spaces.add(fx.spaces_url, fx.spaces_token, "ts-egress")

  assert.deepEqual(info.egress, { allow: [], deny: [] })
  assert.equal(info.egressEnforced, undefined)
  assert.ok(cua.EgressEnforcement.Host !== undefined)
  assert.deepEqual(cua.EgressPolicy.create({}), { allow: [], deny: [] })
  const policy = cua.EgressPolicy.create({ allow: ["1.1.1.1", "2606:4700::1111"], deny: ["all"] })
  assert.deepEqual(policy.deny, ["all"])
  assert.equal(cua.SpaceCreateOptions.create({}).egress, undefined)

  for (const bad of [["example.com"], ["*.example.com"], ["10.0.0.0/33"], ["1.2.3"], [""]]) {
    await assert.rejects(spaces.setEgress(info.id, cua.EgressPolicy.create({ deny: bad })), invalid)
    await assert.rejects(spaces.setEgress(info.id, cua.EgressPolicy.create({ allow: bad })), invalid)
    await assert.rejects(
      spaces.create(cua.SpaceCreateOptions.create({ on: "local", egress: cua.EgressPolicy.create({ deny: bad }) })),
      invalid,
    )
  }
  await assert.rejects(
    spaces.setEgress(info.id, cua.EgressPolicy.create({ deny: ["example.com"] })),
    (e) => invalid(e) && /by IP\/CIDR/.test(e.message),
  )
  for (const extra of [{ durable: true }, { reuse: true }]) {
    await assert.rejects(
      spaces.create(cua.SpaceCreateOptions.create({ on: "local", egress: policy, ...extra })),
      (e) => invalid(e) && /egress/.test(e.message),
    )
  }
  // Cua Cloud has ended: `on: "cloud"` is refused before anything is made.
  await assert.rejects(
    spaces.create(cua.SpaceCreateOptions.create({ on: "cloud", egress: policy })),
    (e) => /Cua Cloud has ended/.test(e.message),
  )
  // A Space added by address is not run by cua: nothing to enforce it.
  await assert.rejects(
    spaces.setEgress(info.id, policy),
    (e) => e instanceof cua.CuaError.Unsupported && /not run by cua/.test(e.message),
  )
  await spaces.delete_(info.id)
})
