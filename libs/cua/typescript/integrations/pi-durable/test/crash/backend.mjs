// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Spaces and snapshots a crash-test process uses, chosen by environment:
//
//   CRASH_BACKEND=docker   (default) Docker containers running a cua-spacesd build
//   CRASH_BACKEND=local    a cua-spacesd stand-in that runs commands on this machine
//                          (test/local-space.mjs): the daemon's exec ledger and
//                          epoch fence on disk under CRASH_LOCAL_DIR; no Docker. The
//                          CI backend; the Space itself cannot be restarted or lost.
//   CRASH_BACKEND=microvm  microVM Spaces managed by the local cua daemon
//                          (CRASH_IMAGE, CRASH_CPUS, CRASH_MEMORY_MB)
//   CRASH_SNAPSHOTS=workspace (default) WorkspaceSnapshots of CRASH_WORK, with a
//                          host CheckpointStore in CRASH_STORE when set
//   CRASH_SNAPSHOTS=machine  SandboxSnapshots: whole-machine checkpoints
//                          (memory and disk on a microVM)

import { mkdtempSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import {
  CuaSandboxSpaces,
  DirectoryCheckpointStore,
  SandboxSnapshots,
  WorkspaceSnapshots,
} from "../../dist/index.js"
import { createHash } from "node:crypto"
import { readdirSync, readFileSync, statSync } from "node:fs"
import { LocalSpacesd, localSpaces } from "../local-space.mjs"
import { DockerSpaces } from "./docker-spaces.mjs"

export const BACKEND = process.env.CRASH_BACKEND ?? "docker"
export const SNAPSHOTS = process.env.CRASH_SNAPSHOTS ?? "workspace"

let daemon
/** The cua daemon's sandboxes (microVM backend). */
export async function daemonSandboxes() {
  if (daemon === undefined) {
    const { connect } = await import("@trycua/cua")
    daemon = connect().sandboxes()
  }
  return daemon
}

/**
 * `{ spaces, snapshots(spaces), restart(name), lose(name), cleanup(name) }`:
 * `snapshots` takes the (possibly traced) Space provider to work through.
 */
export async function backend() {
  if (BACKEND === "local") {
    const spacesd = new LocalSpacesd(join(process.env.CRASH_LOCAL_DIR, "space"))
    const spaces = localSpaces(spacesd, { kind: process.env.CRASH_LOCAL_KIND ?? "container" })
    return {
      spaces,
      spaceFaults: false,
      snapshots: (via) => makeSnapshots(via, undefined),
      async restart() {},
      async lose() {},
      async cleanup() {},
      /** `sha256sum`-style listing of the workspace, computed here (macOS has no sha256sum). */
      async listing(_name, work) {
        const out = []
        const walk = (dir) => {
          for (const name of readdirSync(dir)) {
            const path = join(dir, name)
            if (statSync(path).isDirectory()) walk(path)
            else out.push(`${createHash("sha256").update(readFileSync(path)).digest("hex")}  ./${path.slice(work.length + 1)}`)
          }
        }
        walk(work)
        return out.sort((a, b) => (a.slice(66) < b.slice(66) ? -1 : 1)).join("\n")
      },
    }
  }
  if (BACKEND === "microvm") {
    const { SandboxCreateOptions } = await import("@trycua/cua")
    const sandboxes = await daemonSandboxes()
    const spaces = new CuaSandboxSpaces({
      sandboxes,
      createOptions: (name) =>
        SandboxCreateOptions.create({
          image: process.env.CRASH_IMAGE,
          name,
          kind: "microvm",
          cpus: Number(process.env.CRASH_CPUS ?? 2),
          memoryMb: BigInt(process.env.CRASH_MEMORY_MB ?? 2048),
        }),
    })
    return {
      spaces,
      spaceFaults: true,
      snapshots: (via) => makeSnapshots(via, sandboxes),
      async restart(name) {
        spaces.invalidate(name)
        await (await sandboxes.connect(name)).restart()
      },
      async lose(name) {
        spaces.invalidate(name)
        await sandboxes.delete_(name)
      },
      async cleanup(name) {
        try {
          await sandboxes.delete_(name)
        } catch {
          // Already gone.
        }
        for (const c of await sandboxes.checkpoints()) {
          if (c.name.startsWith(`${name}-`)) await sandboxes.deleteCheckpoint(c.name).catch(() => {})
        }
      },
    }
  }
  if (process.env.CUA_HOME === undefined) {
    process.env.CUA_TEST = "1"
    process.env.CUA_HOME = mkdtempSync(join(tmpdir(), "cua-pi-crash-"))
  }
  const docker = new DockerSpaces({ token: process.env.CRASH_TOKEN })
  return {
    spaces: docker,
    spaceFaults: true,
    snapshots: (via) => makeSnapshots(via, undefined),
    async restart(name) {
      docker.restart(name)
    },
    async lose(name) {
      await docker.remove(name)
    },
    async cleanup(name) {
      await docker.remove(name)
    },
  }
}

function makeSnapshots(spaces, sandboxes) {
  if (SNAPSHOTS === "machine") {
    if (!sandboxes) throw new Error("CRASH_SNAPSHOTS=machine needs CRASH_BACKEND=microvm")
    return new SandboxSnapshots({ sandboxes })
  }
  return new WorkspaceSnapshots({
    spaces,
    root: process.env.CRASH_WORK,
    ...(process.env.CRASH_STORE ? { store: new DirectoryCheckpointStore(process.env.CRASH_STORE) } : {}),
  })
}
