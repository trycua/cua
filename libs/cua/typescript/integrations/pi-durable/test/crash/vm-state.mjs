// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// A whole-machine restore brings back running processes, not only files: a
// counter process started before a microVM checkpoint is killed, the Space
// restored through SandboxSnapshots, and the same process is running again,
// counting on from where the checkpoint caught it.
//
// Usage: CRASH_BACKEND=microvm CRASH_IMAGE=<image> node test/crash/vm-state.mjs   (a cua daemon with microVM support)

import assert from "node:assert/strict"
import { SandboxSnapshots } from "../../dist/index.js"
import { sh } from "../../dist/spacesd.js"
import { backend, daemonSandboxes } from "./backend.mjs"

if (process.env.CRASH_BACKEND !== "microvm") throw new Error("run with CRASH_BACKEND=microvm")
const chosen = await backend()
const sandboxes = await daemonSandboxes()
const snapshots = new SandboxSnapshots({ sandboxes })
const name = `vmstate-${Date.now()}`

async function run(script) {
  const result = await sh(await chosen.spaces.spacesd(name), script)
  assert.equal(result.code, 0, result.stderr)
  return result.stdout.trim()
}

try {
  await chosen.spaces.ensure(name)
  await run(
    "nohup sh -c 'i=0; while :; do i=$((i+1)); echo $i > /tmp/counter; sleep 0.05; done' >/dev/null 2>&1 & echo $! > /tmp/counter.pid",
  )
  await new Promise((resolve) => setTimeout(resolve, 500))
  const pid = await run("cat /tmp/counter.pid")
  let t = performance.now()
  const id = await snapshots.checkpoint(name, `${name}-c1`)
  const checkpointMs = performance.now() - t
  const atCheckpoint = Number(await run("cat /tmp/counter"))
  await run(`kill ${pid}; sleep 0.3; echo dead > /tmp/marker`)
  assert.equal(await run(`kill -0 ${pid} 2>/dev/null && echo alive || echo gone`), "gone")

  t = performance.now()
  await snapshots.restore(name, id)
  chosen.spaces.invalidate(name)
  const restoreMs = performance.now() - t
  const alive = await run(`kill -0 ${pid} 2>/dev/null && echo alive || echo gone`)
  const marker = await run("cat /tmp/marker 2>/dev/null || echo absent")
  const first = Number(await run("cat /tmp/counter"))
  await new Promise((resolve) => setTimeout(resolve, 500))
  const later = Number(await run("cat /tmp/counter"))
  const result = { pid, alive, marker, atCheckpoint, afterRestore: first, halfSecondLater: later, checkpointMs, restoreMs }
  console.log(JSON.stringify(result))
  assert.equal(alive, "alive", "the process from before the checkpoint runs again")
  assert.equal(marker, "absent", "files written after the checkpoint are gone")
  assert.ok(later > first, "it keeps counting")
  console.log("VM STATE OK")
} finally {
  await chosen.cleanup(name)
}
process.exit(0)
