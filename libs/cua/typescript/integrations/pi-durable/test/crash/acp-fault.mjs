// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// Preload for the ACP runner in CRASH_MODE=acp (`NODE_OPTIONS=--import /bundle/acp-fault.mjs`, set by acp.mjs only on
// the drive it attacks). The shipped runner carries no fault hooks; this wraps node's own fs and child_process from
// outside:
//
//   CUA_ACP_TEST_FAULT=after_intent[:n]   SIGKILL right after the n-th `intent` record of tools.jsonl is fsynced: the
//                                         approval is journaled, the harness has no answer yet
//   CUA_ACP_TEST_FAULT=before_intent[:n]  SIGKILL when the n-th permission request arrives from the harness, before the
//                                         runner has looked at it (nothing journaled)
//
// It removes itself from NODE_OPTIONS first, so the harness processes the runner spawns do not load it.

import fs from "node:fs"
import cp from "node:child_process"
import { syncBuiltinESMExports } from "node:module"

delete process.env.NODE_OPTIONS
const [point, count] = String(process.env.CUA_ACP_TEST_FAULT ?? "").split(":")
let left = Number(count) || 1
const die = () => process.kill(process.pid, "SIGKILL")

if (point === "after_intent") {
  const intentFds = new Set()
  const write = fs.writeSync
  fs.writeSync = function (fd, data, ...rest) {
    const out = write.call(this, fd, data, ...rest)
    if (typeof data === "string" && data.includes('"kind":"intent"') && data.includes('"toolCallId"')) intentFds.add(fd)
    return out
  }
  const fsync = fs.fsyncSync
  fs.fsyncSync = function (fd) {
    const out = fsync.call(this, fd)
    if (intentFds.delete(fd) && --left <= 0) die()
    return out
  }
} else if (point === "before_intent") {
  const spawn = cp.spawn
  cp.spawn = function (...args) {
    const child = spawn.apply(this, args)
    child.stdout?.on("data", (chunk) => {
      if (String(chunk).includes('"session/request_permission"') && --left <= 0) die()
    })
    return child
  }
}
// The runner imports `spawn` by name: publish the patched function to the named exports.
syncBuiltinESMExports()
