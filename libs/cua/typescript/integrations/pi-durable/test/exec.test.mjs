// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

import assert from "node:assert/strict"
import { test } from "node:test"
import { execOnSpacesd } from "../dist/exec.js"
import { StaleEpochError } from "../dist/fence.js"
import { abortable, context, exit, FakeProcess, FakeSpacesd, out } from "./fakes.mjs"

function request(overrides = {}) {
  return {
    command: "echo hi",
    options: undefined,
    cwd: "/work",
    shell: ["bash", "-c"],
    requestId: undefined,
    epoch: 0,
    spillTo: undefined,
    append: async () => {},
    running: new Set(),
    ...overrides,
  }
}

test("a string command runs through the shell with the key, epoch and working directory", async () => {
  const spacesd = new FakeSpacesd(() => [out("hi\n"), exit({ code: 0 })])
  const chunks = []
  const result = await execOnSpacesd(
    spacesd,
    request({
      requestId: "7:call-1:0@3",
      epoch: 3,
      options: { env: { A: "1" }, onOutput: (text, _c, info) => chunks.push([info.stream, text]) },
    }),
    context,
  )
  assert.deepEqual(result, { ok: true, value: { exitCode: 0 } })
  const [command] = spacesd.spawned
  assert.equal(command.program, "bash")
  assert.deepEqual(command.args, ["-c", "echo hi"])
  assert.equal(command.cwd, "/work")
  assert.equal(command.requestId, "7:call-1:0@3")
  assert.equal(command.epoch, 3n)
  assert.equal(command.tag, "pi:7:call-1:0@3")
  assert.equal(command.env.get("A"), "1")
  assert.deepEqual(chunks, [["stdout", "hi\n"]])
})

test("output is decoded per stream across chunk boundaries", async () => {
  const euro = new TextEncoder().encode("€")
  const part = (bytes, kind) => ({ kind, offset: 0n, data: bytes.slice().buffer })
  const spacesd = new FakeSpacesd(() => [
    part(euro.subarray(0, 1), 0),
    out("err", 1),
    part(euro.subarray(1), 0),
    exit({ code: 2 }),
  ])
  const text = { stdout: "", stderr: "" }
  const result = await execOnSpacesd(
    spacesd,
    request({ command: ["sh", "-c", "x"], options: { onOutput: (t, _c, i) => (text[i.stream] += t) } }),
    context,
  )
  assert.deepEqual(result, { ok: true, value: { exitCode: 2 } })
  assert.deepEqual(text, { stdout: "€", stderr: "err" })
  assert.equal(spacesd.spawned[0].program, "sh")
})

test("timeouts, missing programs, empty argv and fenced epochs map to execution errors", async () => {
  const timedOut = await execOnSpacesd(
    new FakeSpacesd(() => [exit({ timedOut: true, signal: "term" })]),
    request({ options: { timeout: 0.5 } }),
    context,
  )
  assert.equal(timedOut.error.code, "timeout")
  const missing = await execOnSpacesd(
    new FakeSpacesd(() => [exit({ error: "executable not found: nope" })]),
    request({ command: ["nope"] }),
    context,
  )
  assert.equal(missing.error.code, "spawn_error")
  const empty = new FakeSpacesd(() => [])
  assert.equal((await execOnSpacesd(empty, request({ command: [] }), context)).error.code, "spawn_error")
  assert.equal(empty.spawned.length, 0)
  // A stale epoch is not a command outcome: it halts the driver.
  await assert.rejects(
    execOnSpacesd(
      new FakeSpacesd(() => new Error("epoch 2 is older than the Space's current epoch 3")),
      request({ epoch: 2 }),
      context,
    ),
    (error) => error instanceof StaleEpochError && error.epoch === 2 && error.floor === 3 && error.resource === "space",
  )
  const lost = await execOnSpacesd(
    new FakeSpacesd(() => [exit({ error: "the command was running when cua-spacesd stopped; its outcome is unknown" })]),
    request({ requestId: "k" }),
    context,
  )
  assert.equal(lost.error.code, "unknown")
  const killed = await execOnSpacesd(new FakeSpacesd(() => [exit({ signal: "kill" })]), request(), context)
  assert.deepEqual(killed, { ok: true, value: { exitCode: 137 } })
})

test("aborting kills the command's process group", async () => {
  const process = new FakeProcess([out("start\n")])
  const spacesd = new FakeSpacesd(() => process)
  const { controller, context: aborting } = abortable()
  const running = execOnSpacesd(spacesd, request({ requestId: "r1" }), aborting)
  await new Promise((resolve) => setTimeout(resolve, 10))
  controller.abort()
  await new Promise((resolve) => setTimeout(resolve, 10))
  process.push(exit({ signal: "kill" }))
  const result = await running
  assert.equal(result.error.code, "aborted")
  assert.deepEqual(spacesd.calls[0], [
    "ProcessService/SignalProcess",
    { process: { tag: "pi:r1" }, signal: "SIGNAL_KILL", processGroup: true },
  ])
})

test("output past the spill thresholds goes to a file in the Space, in order", async () => {
  const appended = []
  const spacesd = new FakeSpacesd(() => [out("a\n"), out("b\n"), out("c\n"), exit({ code: 0 })])
  const result = await execOnSpacesd(
    spacesd,
    request({
      options: { spill: { afterBytes: 100, afterLines: 1 } },
      spillTo: async () => "/tmp/pi-output-1.log",
      append: async (path, bytes) => appended.push([path, new TextDecoder().decode(bytes)]),
    }),
    context,
  )
  assert.deepEqual(result, { ok: true, value: { exitCode: 0, spillPath: "/tmp/pi-output-1.log" } })
  assert.equal(appended.map(([, text]) => text).join(""), "a\nb\nc\n")
  assert.ok(appended.every(([path]) => path === "/tmp/pi-output-1.log"))
})

test("without inherited environment the command runs under env -i", async () => {
  const spacesd = new FakeSpacesd(() => [exit({ code: 0 })])
  await execOnSpacesd(
    spacesd,
    request({ command: ["printenv"], options: { inheritEnv: false, env: { ONLY: "this" } } }),
    context,
  )
  const [command] = spacesd.spawned
  assert.equal(command.program, "env")
  assert.deepEqual(command.args, ["-i", "ONLY=this", "printenv"])
  assert.equal(command.env.size, 0)
})
