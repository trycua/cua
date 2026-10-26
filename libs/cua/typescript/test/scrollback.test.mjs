// `SpacesdCommand.scrollbackBytes` sizes the guest's output scrollback ring.
//
// The guest keeps a process's output in a bounded ring and never blocks the
// process, so a reader that falls behind by more than the ring loses the
// evicted bytes (an offset gap in ProcessEvent.offset). Against the fixture's
// in-process cua-spacesd core (real child processes), a consumer that starts
// reading only after the process finished still gets all 16 MiB, in order,
// with a 64 MiB ring; with a 1 MiB ring the same reader sees the gap.
import assert from "node:assert/strict"
import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { after, before, test } from "node:test"

import * as cua from "../dist/index.js"
import { binary, startFixtures } from "./fixtures.mjs"

const MIB = 1024 * 1024
const SIZE = 16 * MIB

let fx
before(async () => {
  fx = await startFixtures()
})
after(async () => {
  await fx?.stop()
})

const producer = (scrollbackBytes) =>
  cua.SpacesdCommand.create({
    program: "sh",
    args: ["-c", `yes 0123456789abcdefghijklmnopqrstuvwxyz | head -c ${SIZE}`],
    scrollbackBytes,
  })

// Reads after the process finished, then slowly.
async function slowRead(proc) {
  await new Promise((r) => setTimeout(r, 2000))
  let got = 0
  let gap
  for (;;) {
    const ev = await proc.nextEvent()
    assert.ok(ev, "no exit event")
    if (ev.kind === cua.ProcessEventKind.Exit) return { got, gap, exit: ev.exit }
    if (Number(ev.offset) !== got && gap === undefined) gap = [got, Number(ev.offset)]
    got += ev.data.byteLength
    if (got % (4 * MIB) < ev.data.byteLength) await new Promise((r) => setTimeout(r, 200))
  }
}

test("scrollbackBytes defaults to undefined", () => {
  assert.equal(cua.SpacesdCommand.create({ program: "x" }).scrollbackBytes, undefined)
  assert.equal(cua.SpacesdCommand.create({ program: "x", scrollbackBytes: 5n }).scrollbackBytes, 5n)
})

test(
  "large output read slowly arrives whole",
  { skip: !binary("CUA_TEST_FIXTURES", "cua-test-fixtures"), timeout: 120_000 },
  async () => {
    const dir = mkdtempSync(join(tmpdir(), "cua-ts-scrollback-"))
    const c = cua.embedded({ stateDir: join(dir, "sbx"), spacesHome: join(dir, "cua") })
    const env = await c.spacesd(fx.spaces_url, fx.spaces_token)

    let r = await slowRead(await env.spawn(producer(BigInt(64 * MIB))))
    assert.ok(r.exit.success)
    assert.equal(r.gap, undefined, `offset gap ${r.gap}`)
    assert.equal(r.got, SIZE)

    // Control: a 1 MiB ring loses what the slow reader did not take in time.
    r = await slowRead(await env.spawn(producer(BigInt(MIB))))
    assert.ok(r.exit.success)
    assert.ok(r.gap !== undefined && r.got < SIZE)

    // Over the server maximum is clamped, not refused.
    r = await slowRead(await env.spawn(producer(2n ** 40n)))
    assert.equal(r.gap, undefined)
    assert.equal(r.got, SIZE)
  },
)
