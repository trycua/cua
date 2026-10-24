// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Where Spaces come from and how their state is captured.
 *
 * `SpaceProvider` names Spaces and connects to them; `SnapshotProvider`
 * checkpoints, restores and forks them. Both are small so a host can back them
 * with any runtime: `CuaSandboxSpaces` uses the cua SDK's sandboxes, and
 * `WorkspaceSnapshots` captures one directory tree of the Space through
 * cua-spacesd itself, which works on every Space kind. A whole-machine
 * provider (disk, and with microVMs memory) plugs in through the same
 * interface.
 */

import { mkdir, open, readFile, rename, rm, stat } from "node:fs/promises"
import { join } from "node:path"
import { type SpacesdLike, quote, sh } from "./spacesd.js"

export interface SpaceProvider {
  /** Whether the Space `name` exists. */
  exists(name: string): Promise<boolean>
  /** Create `name` from the provider's base image when it does not exist. Idempotent. */
  ensure(name: string): Promise<void>
  /** cua-spacesd of `name`. */
  spacesd(name: string): Promise<SpacesdLike>
  /** Delete `name`. */
  remove?(name: string): Promise<void>
  /** Forget cached connections to `name`: it was replaced (restored or rebuilt). */
  invalidate?(name: string): void
  /**
   * The runtime kind of `name` (`container`, `gvisor`, `microvm`, `vm`, ...),
   * when the provider knows it. `durableSpaces({ mode: "auto" })` uses it to
   * decide between transactional and checkpoint mode.
   */
  kind?(name: string): Promise<string | undefined>
}

export interface SnapshotProvider {
  /**
   * Capture the current state of `space` as checkpoint `label` and return its
   * id. Idempotent by label: an existing checkpoint is kept and returned. The
   * caller guarantees nothing mutates the Space meanwhile.
   */
  checkpoint(space: string, label: string): Promise<string>
  /** Replace the state of `space` with checkpoint `id`. Idempotent. */
  restore(space: string, id: string): Promise<void>
  /**
   * Create Space `target` from checkpoint `id` taken of `source`. Afterwards
   * `restore(target, id)` must work too. Idempotent: calling it again
   * (for example after a crash halfway) leaves `target` in that state.
   */
  fork(id: string, source: string, target: string): Promise<void>
  /** Delete checkpoint `id` of `space`. */
  remove?(space: string, id: string): Promise<void>
  /**
   * True when a checkpoint captures the whole machine (disk and, on a microVM,
   * memory and processes), as `SandboxSnapshots` does. `mode: "auto"` keeps
   * checkpointing such Spaces when they are VMs.
   */
  readonly wholeMachine?: boolean
}

/** Checkpoint labels: lowercase letters, digits, `-`, at most 64 characters. */
export function checkpointLabel(...parts: string[]): string {
  return parts
    .join("-")
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, "-")
    .replace(/-+/g, "-")
    .replace(/^-|-$/g, "")
    .slice(0, 64)
}

/**
 * Where checkpoint archives are kept outside the Space, so a conversation can
 * resume on another Space (another machine) after its own is lost.
 */
export interface CheckpointStore {
  /** Whether `id` is stored. */
  has(id: string): Promise<boolean>
  /** Store `bytes` as `id`, durably, before resolving. Idempotent. */
  put(id: string, bytes: Uint8Array): Promise<void>
  /** The archive `id`, or undefined. */
  get(id: string): Promise<Uint8Array | undefined>
  remove?(id: string): Promise<void>
}

/** A `CheckpointStore` in a local directory (a mounted volume or synced folder makes it shared). */
export class DirectoryCheckpointStore implements CheckpointStore {
  readonly #dir: string

  constructor(dir: string) {
    this.#dir = dir
  }

  #path(id: string): string {
    if (!/^[a-z0-9][a-z0-9-]*$/.test(id)) throw new Error(`invalid checkpoint id ${JSON.stringify(id)}`)
    return join(this.#dir, `${id}.tar`)
  }

  async has(id: string): Promise<boolean> {
    try {
      await stat(this.#path(id))
      return true
    } catch {
      return false
    }
  }

  async put(id: string, bytes: Uint8Array): Promise<void> {
    await mkdir(this.#dir, { recursive: true })
    const path = this.#path(id)
    const staging = `${path}.${process.pid}.tmp`
    const file = await open(staging, "w")
    try {
      await file.writeFile(bytes)
      await file.sync()
    } finally {
      await file.close()
    }
    await rename(staging, path)
  }

  async get(id: string): Promise<Uint8Array | undefined> {
    try {
      return new Uint8Array(await readFile(this.#path(id)))
    } catch {
      return undefined
    }
  }

  async remove(id: string): Promise<void> {
    await rm(this.#path(id), { force: true })
  }
}

export interface WorkspaceSnapshotsOptions {
  spaces: SpaceProvider
  /** The directory tree that is the Space's state, e.g. the agent's working directory. */
  root: string
  /** Where checkpoints are kept in the Space. Default `/var/tmp/cua-checkpoints`. */
  guestDir?: string
  /**
   * A copy of every checkpoint outside the Space. With it, a lost Space can be
   * rebuilt from its newest checkpoint on any machine, and forks do not read
   * from the parent's Space.
   */
  store?: CheckpointStore
}

function bytesOf(data: ArrayBuffer | Uint8Array): ArrayBuffer {
  if (data instanceof ArrayBuffer) return data
  return data.buffer.slice(data.byteOffset, data.byteOffset + data.byteLength) as ArrayBuffer
}

/**
 * Checkpoints of one directory tree (`root`), stored as tar archives inside
 * the Space (and optionally copied to a `CheckpointStore`), taken through
 * cua-spacesd. Restoring replaces the tree's contents in place (the directory
 * itself keeps its identity). State outside `root` (installed packages, other
 * directories, running processes) is not captured; use a whole-machine
 * provider for that.
 */
export class WorkspaceSnapshots implements SnapshotProvider {
  readonly #spaces: SpaceProvider
  readonly #root: string
  readonly #guestDir: string
  readonly #store: CheckpointStore | undefined

  constructor(options: WorkspaceSnapshotsOptions) {
    this.#spaces = options.spaces
    this.#root = options.root.replace(/\/+$/, "") || "/"
    this.#guestDir = options.guestDir ?? "/var/tmp/cua-checkpoints"
    this.#store = options.store
  }

  #archive(id: string): string {
    if (!/^[a-z0-9][a-z0-9-]*$/.test(id)) throw new Error(`invalid checkpoint id ${JSON.stringify(id)}`)
    return `${this.#guestDir}/${id}.tar`
  }

  async #run(spacesd: SpacesdLike, script: string, what: string): Promise<void> {
    const result = await sh(spacesd, `set -e\n${script}`)
    if (result.code !== 0) {
      throw new Error(`${what} failed (${result.code ?? result.error}): ${result.stderr.trim().slice(-500)}`)
    }
  }

  /** Puts the archive `id` into the Space from `bytes`. */
  async #place(spacesd: SpacesdLike, id: string, bytes: ArrayBuffer | Uint8Array, space: string): Promise<void> {
    const archive = quote(this.#archive(id))
    // A staging name of its own: a stale driver and its successor can both be placing the same archive.
    const staging = `${this.#archive(id)}.${process.pid}.${Math.random().toString(36).slice(2, 10)}.tmp`
    await this.#run(spacesd, `mkdir -p ${quote(this.#guestDir)}`, `prepare ${space}`)
    await spacesd.upload(staging, bytesOf(bytes), {
      createNew: false,
      append: false,
      permissions: 0o644,
      createParents: true,
    })
    await this.#run(spacesd, `mv -f ${quote(staging)} ${archive}`, `store ${id} in ${space}`)
  }

  async checkpoint(space: string, label: string): Promise<string> {
    const id = checkpointLabel(label)
    const archive = quote(this.#archive(id))
    const spacesd = await this.#spaces.spacesd(space)
    await this.#run(
      spacesd,
      `mkdir -p ${quote(this.#guestDir)} ${quote(this.#root)}
if [ -f ${archive} ]; then exit 0; fi
# Staged under a name of its own ($$): a stale driver and its successor can checkpoint at once.
tmp=${archive}.$$.tmp
tar -C ${quote(this.#root)} -cf "$tmp" .
sync "$tmp" 2>/dev/null || sync
mv -f "$tmp" ${archive}`,
      `checkpoint ${id} of ${space}`,
    )
    if (this.#store && !(await this.#store.has(id))) {
      await this.#store.put(id, new Uint8Array(await spacesd.download(this.#archive(id))))
    }
    return id
  }

  async restore(space: string, id: string): Promise<void> {
    const archive = quote(this.#archive(id))
    const root = quote(this.#root)
    const spacesd = await this.#spaces.spacesd(space)
    if (this.#store) {
      const present = await sh(spacesd, `test -f ${archive}`)
      if (present.code !== 0) {
        const bytes = await this.#store.get(id)
        if (bytes === undefined) throw new Error(`checkpoint ${id} is neither in ${space} nor in the store`)
        await this.#place(spacesd, id, bytes, space)
      }
    }
    await this.#run(
      spacesd,
      `test -f ${archive}
mkdir -p ${root}
find ${root} -mindepth 1 -maxdepth 1 -exec rm -rf {} +
tar -C ${root} -xpf ${archive}
sync`,
      `restore ${id} into ${space}`,
    )
  }

  async fork(id: string, source: string, target: string): Promise<void> {
    // Idempotent: a fork cut off halfway is completed, not skipped.
    await this.#spaces.ensure(target)
    const to = await this.#spaces.spacesd(target)
    if ((await sh(to, `test -f ${quote(this.#archive(id))}`)).code !== 0) {
      const stored = this.#store ? await this.#store.get(id) : undefined
      const bytes = stored ?? (await (await this.#spaces.spacesd(source)).download(this.#archive(id)))
      await this.#place(to, id, bytes, target)
    }
    await this.restore(target, id)
  }

  async remove(space: string, id: string): Promise<void> {
    const spacesd = await this.#spaces.spacesd(space)
    await this.#run(spacesd, `rm -f ${quote(this.#archive(id))}`, `remove ${id}`)
    await this.#store?.remove?.(id)
  }
}

/** The parts of the cua SDK (`@trycua/cua`) `CuaSandboxSpaces` uses. */
export interface CuaSandboxesLike {
  get(name: string): Promise<unknown>
  create(options: unknown): Promise<unknown>
  connect(name: string): Promise<{ spacesd(probeTimeoutMs: number | undefined): Promise<SpacesdLike> }>
  delete_(name: string): Promise<void>
}

export interface CuaSandboxSpacesOptions {
  /** `embedded().sandboxes()` or `connect().sandboxes()` from `@trycua/cua`. */
  sandboxes: CuaSandboxesLike
  /** Builds the create options of a new Space (`SandboxCreateOptions.create({...})`). */
  createOptions: (name: string) => unknown
}

/** Spaces as cua sandboxes: one sandbox per name, created on demand. */
export class CuaSandboxSpaces implements SpaceProvider {
  readonly #options: CuaSandboxSpacesOptions
  readonly #clients = new Map<string, Promise<SpacesdLike>>()

  constructor(options: CuaSandboxSpacesOptions) {
    this.#options = options
  }

  async exists(name: string): Promise<boolean> {
    try {
      await this.#options.sandboxes.get(name)
      return true
    } catch (error) {
      if (/not ?found|NotFound|no such|unknown sandbox/i.test(String((error as Error)?.message ?? error))) return false
      const tag = (error as { tag?: string })?.tag
      if (tag === "NotFound" || tag === "SandboxNotFound") return false
      throw error
    }
  }

  async ensure(name: string): Promise<void> {
    if (await this.exists(name)) return
    try {
      await this.#options.sandboxes.create(this.#options.createOptions(name))
    } catch (error) {
      // A concurrent create of the same name won the race.
      if (!(await this.exists(name))) throw error
    }
  }

  spacesd(name: string): Promise<SpacesdLike> {
    let client = this.#clients.get(name)
    if (client === undefined) {
      client = this.#connect(name)
      this.#clients.set(name, client)
      client.catch(() => this.#clients.delete(name))
    }
    return client
  }

  /**
   * cua-spacesd of `name`, waiting (up to a minute) while it starts: a Space
   * that was just restarted, created or restored may not answer yet.
   */
  async #connect(name: string): Promise<SpacesdLike> {
    const deadline = Date.now() + 60_000
    for (let delay = 250; ; delay = Math.min(delay * 2, 2_000)) {
      try {
        const sandbox = await this.#options.sandboxes.connect(name)
        return await sandbox.spacesd(60_000)
      } catch (error) {
        const tag = (error as { tag?: string })?.tag
        if (tag !== "SpacesdNotAvailable" || Date.now() + delay > deadline) throw error
        await new Promise((resolve) => setTimeout(resolve, delay))
      }
    }
  }

  async remove(name: string): Promise<void> {
    this.#clients.delete(name)
    await this.#options.sandboxes.delete_(name)
  }

  invalidate(name: string): void {
    this.#clients.delete(name)
  }

  async kind(name: string): Promise<string | undefined> {
    try {
      const sandbox = (await this.#options.sandboxes.get(name)) as { kind?: unknown } | undefined
      const kind = typeof sandbox?.kind === "function" ? await (sandbox.kind as () => unknown).call(sandbox) : sandbox?.kind
      return typeof kind === "string" ? kind : undefined
    } catch {
      return undefined
    }
  }
}

/** The checkpoint calls of the cua SDK's `Sandboxes` (`@trycua/cua`). */
export interface CuaCheckpointsLike {
  checkpoint(name: string, checkpoint: string): Promise<unknown>
  fork(source: string, name: string): Promise<unknown>
  checkpoints(): Promise<Array<{ name: string }>>
  deleteCheckpoint(name: string): Promise<void>
  get(name: string): Promise<unknown>
  delete_(name: string): Promise<void>
}

/**
 * Whole-machine checkpoints of cua sandboxes (`cua sb checkpoint` / `fork`):
 * memory and disk on a microVM, the filesystem of a container. A restore
 * replaces the Space with a fork of the checkpoint under the same name, so
 * running processes come back as they were. Checkpoints live with the cua
 * daemon, outside the Space, so a lost Space is rebuilt from them.
 */
export class SandboxSnapshots implements SnapshotProvider {
  readonly wholeMachine = true
  readonly #sandboxes: CuaCheckpointsLike

  constructor(options: { sandboxes: CuaCheckpointsLike }) {
    this.#sandboxes = options.sandboxes
  }

  async #has(id: string): Promise<boolean> {
    return (await this.#sandboxes.checkpoints()).some((c) => c.name === id)
  }

  async #exists(name: string): Promise<boolean> {
    try {
      await this.#sandboxes.get(name)
      return true
    } catch {
      return false
    }
  }

  async checkpoint(space: string, label: string): Promise<string> {
    const id = checkpointLabel(label)
    if (await this.#has(id)) return id
    try {
      await this.#sandboxes.checkpoint(space, id)
    } catch (error) {
      // A concurrent or interrupted attempt that finished.
      if (!(await this.#has(id))) throw error
    }
    return id
  }

  async restore(space: string, id: string): Promise<void> {
    if (!(await this.#has(id))) throw new Error(`no checkpoint ${id}`)
    if (await this.#exists(space)) await this.#sandboxes.delete_(space)
    await this.#sandboxes.fork(id, space)
  }

  async fork(id: string, source: string, target: string): Promise<void> {
    if (source === target) {
      // Rebuilding a lost Space: whatever is left under the name is replaced.
      await this.restore(target, id)
      return
    }
    if (await this.#exists(target)) return
    await this.#sandboxes.fork(id, target)
  }

  async remove(_space: string, id: string): Promise<void> {
    await this.#sandboxes.deleteCheckpoint(id)
  }
}
