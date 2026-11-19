// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// Latency of the pieces durableSpaces adds per mutating call, on one Space:
// a plain command, a keyed (ledger) command, a replayed keyed command, a
// workspace checkpoint (with and without a host copy) and a restore, for a
// small and a larger workspace. Prints JSON.
//
// Usage: node test/crash/bench.mjs [iterations=20]   (Docker, like run.mjs)

import { randomBytes, randomUUID } from "node:crypto"
import { mkdtempSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { BACKGROUND_CONTEXT as context } from "@earendil-works/chord/context"
import { DirectoryCheckpointStore, SpacesExecutionEnv, WorkspaceSnapshots } from "../../dist/index.js"
import { sh } from "../../dist/spacesd.js"
import { DockerSpaces } from "./docker-spaces.mjs"

const N = Number(process.argv[2] ?? 20)
const dir = mkdtempSync(join(tmpdir(), "pi-bench-"))
process.env.CUA_HOME = join(dir, "cua-home")
const spaces = new DockerSpaces({ token: randomBytes(16).toString("hex") })
const name = `bench-${Date.now()}`

function stats(samples) {
  const sorted = [...samples].sort((a, b) => a - b)
  const at = (q) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))]
  return { n: sorted.length, p50: +at(0.5).toFixed(2), p90: +at(0.9).toFixed(2), max: +sorted.at(-1).toFixed(2) }
}

async function time(fn) {
  const t = performance.now()
  await fn()
  return performance.now() - t
}

try {
  await spaces.ensure(name)
  const spacesd = await spaces.spacesd(name)
  const results = { iterations: N }
  const env = new SpacesExecutionEnv({ spacesd, id: `cua-space:${name}`, cwd: "/work", epoch: () => 1 })
  const ok = (r) => {
    if (!r.ok || r.value.exitCode !== 0) throw new Error(JSON.stringify(r))
  }
  const plain = []
  const keyed = []
  const replayed = []
  for (let i = 0; i < N; i++) {
    plain.push(await time(async () => ok(await env.exec("echo hi", undefined, context))))
    const prefix = `bench:${randomUUID()}`
    keyed.push(await time(async () => ok(await env.withRequestIds(prefix).exec("echo hi", undefined, context))))
    replayed.push(await time(async () => ok(await env.withRequestIds(prefix).exec("echo hi", undefined, context))))
  }
  results.exec = { plain: stats(plain), keyed: stats(keyed), keyedReplay: stats(replayed) }

  for (const [label, files, size] of [
    ["small (10 files, 4 KiB each)", 10, 4096],
    ["medium (500 files, 16 KiB each)", 500, 16384],
  ]) {
    await sh(spacesd, `rm -rf /work/* && mkdir -p /work/src && for i in $(seq 1 ${files}); do head -c ${size} /dev/urandom > /work/src/f$i; done`)
    const local = new WorkspaceSnapshots({ spaces, root: "/work" })
    const stored = new WorkspaceSnapshots({
      spaces,
      root: "/work",
      store: new DirectoryCheckpointStore(join(dir, "store")),
    })
    const checkpoint = []
    const checkpointStored = []
    const restore = []
    for (let i = 0; i < N; i++) {
      checkpoint.push(await time(() => local.checkpoint(name, `b-${randomUUID()}`)))
      const id = `s-${randomUUID()}`
      checkpointStored.push(await time(() => stored.checkpoint(name, id)))
      restore.push(await time(() => local.restore(name, id)))
    }
    results[label] = {
      checkpoint: stats(checkpoint),
      checkpointWithHostCopy: stats(checkpointStored),
      restore: stats(restore),
    }
    await sh(spacesd, "rm -rf /var/tmp/cua-checkpoints/*")
  }
  await env.close()
  console.log(JSON.stringify(results, null, 1))
} finally {
  await spaces.remove(name)
  rmSync(dir, { recursive: true, force: true })
}
process.exit(0)
