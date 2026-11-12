import assert from "node:assert/strict"
import { test } from "node:test"
import { createDriver, resetDaemonWarning } from "../dist/daemon.js"

const fake = (name, upAfter = 0) => {
  let calls = 0
  return { name, calls: () => calls, isAvailable: () => ++calls > upAfter }
}
const base = { platform: "darwin", sleep() {} }

test("a running daemon is used on macOS", () => {
  delete process.env.CUA_DRIVER_EMBEDDED
  const daemon = fake("daemon")
  const embedded = fake("embedded")
  assert.equal(createDriver(() => daemon, () => embedded, false, false, base), daemon)
})

test("an installed app is launched and awaited", () => {
  delete process.env.CUA_DRIVER_EMBEDDED
  const daemon = fake("daemon", 2)
  let launched = 0
  const result = createDriver(() => daemon, () => fake("e"), false, false, {
    ...base,
    appInstalled: () => true,
    launch: () => (launched++, true),
  })
  assert.equal(result, daemon)
  assert.equal(launched, 1)
})

test("fallback to embedded warns once", () => {
  delete process.env.CUA_DRIVER_EMBEDDED
  resetDaemonWarning()
  const warnings = []
  const embedded = fake("embedded")
  for (let i = 0; i < 2; i++) {
    const result = createDriver(() => fake("d", 1e9), () => embedded, false, false, {
      ...base,
      appInstalled: () => false,
      warn: (m) => warnings.push(m),
    })
    assert.equal(result, embedded)
  }
  assert.equal(warnings.length, 1)
  assert.match(warnings[0], /agent cursor/)
})

test("embedded is forced by option, env, compat flag or non-macOS", () => {
  delete process.env.CUA_DRIVER_EMBEDDED
  const daemon = fake("daemon")
  const embedded = fake("embedded")
  const connect = () => daemon
  const create = () => embedded
  assert.equal(createDriver(connect, create, true, false, base), embedded)
  assert.equal(createDriver(connect, create, false, true, base), embedded)
  assert.equal(createDriver(connect, create, false, false, { platform: "linux" }), embedded)
  assert.equal(createDriver(connect, create, false, false, { platform: "win32" }), embedded)
  process.env.CUA_DRIVER_EMBEDDED = "1"
  assert.equal(createDriver(connect, create, false, false, base), embedded)
  delete process.env.CUA_DRIVER_EMBEDDED
  assert.equal(daemon.calls(), 0)
})
