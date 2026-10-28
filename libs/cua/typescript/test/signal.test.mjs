// `ExitInfo.signalNumber` / `signalName` report any terminating signal.
//
// `ExitInfo.signal` only names the signals of the contract enum (kill, term,
// ...). A process that dies of SIGSEGV, SIGABRT or SIGPIPE leaves it unset;
// the raw number and the conventional name say what happened. Against the
// fixture's in-process cua-spacesd core (real child processes).
import assert from "node:assert/strict"
import { mkdtempSync } from "node:fs"
import { constants, tmpdir } from "node:os"
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

async function runSh(env, script) {
  const proc = await env.spawn(cua.SpacesdCommand.create({ program: "sh", args: ["-c", script] }))
  for (;;) {
    const ev = await proc.nextEvent()
    assert.ok(ev, "no exit event")
    if (ev.kind === cua.ProcessEventKind.Exit) return ev.exit
  }
}

test("signal fields default to undefined", () => {
  const info = cua.ExitInfo.create({ code: 0, timedOut: false, success: true })
  assert.equal(info.signalNumber, undefined)
  assert.equal(info.signalName, undefined)
})

test(
  "signal deaths carry number and name",
  { skip: !binary("CUA_TEST_FIXTURES", "cua-test-fixtures"), timeout: 60_000 },
  async () => {
    const dir = mkdtempSync(join(tmpdir(), "cua-ts-signal-"))
    const c = cua.embedded({ stateDir: join(dir, "sbx"), spacesHome: join(dir, "cua") })
    const env = await c.spacesd(fx.spaces_url, fx.spaces_token)

    // Outside the contract enum: only the raw fields say which signal.
    for (const name of ["SEGV", "ABRT", "PIPE"]) {
      const exit = await runSh(env, `kill -${name} $$; sleep 5`)
      assert.equal(exit.code, undefined)
      assert.equal(exit.success, false)
      assert.equal(exit.signal, undefined)
      assert.equal(exit.signalName, name)
      assert.equal(exit.signalNumber, constants.signals["SIG" + name])
    }

    // Inside the enum: both are set.
    const killed = await runSh(env, "kill -KILL $$; sleep 5")
    assert.equal(killed.signal, "kill")
    assert.equal(killed.signalNumber, 9)
    assert.equal(killed.signalName, "KILL")

    // A normal exit has no signal at all.
    const ok = await runSh(env, "exit 3")
    assert.equal(ok.code, 3)
    assert.equal(ok.signalNumber, undefined)
    assert.equal(ok.signalName, undefined)
  },
)
