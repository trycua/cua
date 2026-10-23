// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// Checkpoint and restore latency of the two snapshot providers on the same
// microVM Space: WorkspaceSnapshots (a tar of one directory through
// cua-spacesd, with and without a host copy) and SandboxSnapshots (the whole
// machine, memory and disk). Prints JSON.
//
// Usage: CRASH_BACKEND=microvm CRASH_IMAGE=<image> node test/crash/bench-providers.mjs [iterations=10]

import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { DirectoryCheckpointStore, SandboxSnapshots, WorkspaceSnapshots } from "../../dist/index.js"
import { sh } from "../../dist/spacesd.js"
import { backend, daemonSandboxes } from "./backend.mjs"

if (process.env.CRASH_BACKEND !== "microvm") throw new Error("run with CRASH_BACKEND=microvm")
const N = Number(process.argv[2] ?? 10)
const chosen = await backend()
const sandboxes = await daemonSandboxes()
const name = `bench-prov-${Date.now()}`
const root = "/home/cua/work"
const dir = mkdtempSync(join(tmpdir(), "pi-bench-prov-"))

const stats = (samples) => {
  const s = [...samples].sort((a, b) => a - b)
  const at = (q) => +s[Math.min(s.length - 1, Math.floor(q * s.length))].toFixed(1)
  return { n: s.length, p50: at(0.5), p90: at(0.9), max: +s.at(-1).toFixed(1) }
}
const time = async (fn) => {
  const t = performance.now()
  await fn()
  return performance.now() - t
}

try {
  await chosen.spaces.ensure(name)
  const spacesd = await chosen.spaces.spacesd(name)
  const fill = await sh(spacesd, `mkdir -p ${root} && for i in $(seq 1 10); do head -c 4096 /dev/urandom > ${root}/f$i; done`)
  if (fill.code !== 0) throw new Error(fill.stderr)
  const providers = {
    workspace: new WorkspaceSnapshots({ spaces: chosen.spaces, root }),
    workspaceWithHostCopy: new WorkspaceSnapshots({
      spaces: chosen.spaces,
      root,
      store: new DirectoryCheckpointStore(join(dir, "store")),
    }),
    machine: new SandboxSnapshots({ sandboxes }),
  }
  const results = { iterations: N, files: "10 files of 4 KiB", space: "microVM, 2 vCPU, 2 GiB" }
  for (const [label, provider] of Object.entries(providers)) {
    const checkpoint = []
    const restore = []
    for (let i = 0; i < N; i++) {
      let id
      checkpoint.push(await time(async () => (id = await provider.checkpoint(name, `${name}-${label}-${i}`))))
      restore.push(
        await time(async () => {
          await provider.restore(name, id)
          chosen.spaces.invalidate(name)
          // Ready for the next command: the Space answers again.
          const ok = await sh(await chosen.spaces.spacesd(name), "true")
          if (ok.code !== 0) throw new Error("not ready")
        }),
      )
    }
    results[label] = { checkpoint: stats(checkpoint), restoreUntilReady: stats(restore) }
  }
  console.log(JSON.stringify(results, null, 1))
} finally {
  await chosen.cleanup(name)
  rmSync(dir, { recursive: true, force: true })
}
process.exit(0)
