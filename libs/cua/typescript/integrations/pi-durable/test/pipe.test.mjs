// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { test } from "node:test"
import { GuestPipe } from "../dist/pipe.js"
import { exit, FakeProcess, FakeSpacesd, out } from "./fakes.mjs"

test("a local child's stdio reaches the guest process byte for byte", async () => {
  let guest
  const spacesd = new FakeSpacesd(() => {
    // Echoes stdin back on stdout; exits on "bye".
    guest = new FakeProcess([], {
      stdin: (bytes, self) => {
        const text = new TextDecoder().decode(bytes)
        self.push(out(text))
        if (text.includes("bye")) self.push(exit({ code: 0 }))
      },
    })
    return guest
  })
  const pipe = new GuestPipe({
    spacesd: async () => spacesd,
    program: async () => "/var/tmp/cua-pi-env/pi-env",
    cwd: "/work",
    epoch: () => 4,
  })
  try {
    const [program, ...args] = await pipe.command()
    const child = spawn(program, [...args, "serve", "--token", "abc"], { stdio: ["pipe", "pipe", "inherit"] })
    let echoed = ""
    child.stdout.on("data", (chunk) => (echoed += chunk.toString("utf8")))
    const exited = new Promise((resolve) => child.on("exit", resolve))
    child.stdin.write("hello ")
    await new Promise((resolve) => setTimeout(resolve, 100))
    child.stdin.write("bye")
    assert.equal(await exited, 0)
    assert.equal(echoed, "hello bye")
    const [command] = spacesd.spawned
    assert.equal(command.program, "/var/tmp/cua-pi-env/pi-env")
    assert.deepEqual(command.args, ["serve", "--token", "abc"])
    assert.equal(command.stdin, true)
    assert.equal(command.epoch, 4n)
    assert.equal(command.cwd, "/work")
    assert.match(command.tag, /^pi-env:/)
  } finally {
    await pipe.close()
  }
})
