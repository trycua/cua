// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Epoch fencing for the places a durable agent writes to.
 *
 * A session's epoch only grows: the coordinator issues a new one for every
 * drive attempt, and the new driver raises the floor of every resource it
 * writes to, which cuts off any older driver still running. A write whose
 * epoch is below the floor is refused with `StaleEpochError`, and the driver
 * must stop instead of recording the refusal as a result.
 *
 * - `FencedSqliteDatabase` keeps the floor in a one-row table of the same
 *   SQLite file as the log and checks it inside every transaction, so the
 *   check and the commit are one atomic step.
 * - `FencedFileSystem` keeps the floor as marker files next to a file-based
 *   log. A filesystem has no transactions, so its fence is weaker; see the
 *   class for exactly how.
 */

import { BACKGROUND_CONTEXT } from "@earendil-works/chord/context"
import type { Context } from "@earendil-works/chord"
import type { FileSystem } from "@earendil-works/pi-durable/env"
import {
  type SqliteDatabase,
  type SqliteExecutor,
  type SqliteValue,
  SqliteStorage,
} from "@earendil-works/pi-durable/storage/sqlite"
import { type NodeSqliteStorageOptions, openNodeSqliteDatabase } from "@earendil-works/pi-durable/storage/sqlite/node"

/** A write carried an epoch below the resource's floor: a newer driver owns the session. */
export class StaleEpochError extends Error {
  override readonly name = "StaleEpochError"
  /** The epoch of the refused writer. */
  readonly epoch: number
  /** The floor it ran into (0 when the resource did not say). */
  readonly floor: number
  /** What refused it: `log`, `space`, or the path of a file log. */
  readonly resource: string

  constructor(epoch: number, floor: number, resource: string, options?: { cause?: unknown }) {
    super(
      `stale epoch: writer epoch ${epoch} is older than the ${resource} floor${floor > 0 ? ` ${floor}` : ""}; a newer driver owns this session`,
      options,
    )
    this.epoch = epoch
    this.floor = floor
    this.resource = resource
  }
}

/**
 * The driver cannot continue and must stop, but not because it is stale: the
 * Space refused or failed an epoch operation in a way resuming alone will not
 * fix (for example the SDK or cua-spacesd lacks `AdvanceEpoch`), or in a way
 * that leaves the fence unestablished. `permanent` says whether a new attempt
 * can help (a lost connection) or not (a missing capability).
 */
export class HaltError extends Error {
  override readonly name = "HaltError"
  readonly permanent: boolean
  constructor(message: string, permanent: boolean, options?: { cause?: unknown }) {
    super(message, options)
    this.permanent = permanent
  }
}

/** The `HaltError` inside `error`'s cause chain, or undefined. */
export function haltOf(error: unknown): HaltError | undefined {
  for (let e: unknown = error, depth = 0; e !== undefined && e !== null && depth < 8; depth++) {
    if (e instanceof HaltError || (e as { name?: unknown }).name === "HaltError") return e as HaltError
    e = (e as { cause?: unknown }).cause
  }
  return undefined
}

/** Whether `error` is, or was caused by, a `StaleEpochError`. */
export function isStaleEpochError(error: unknown): error is StaleEpochError {
  for (let e: unknown = error, depth = 0; e !== undefined && e !== null && depth < 8; depth++) {
    if (e instanceof StaleEpochError || (e as { name?: unknown }).name === "StaleEpochError") return true
    e = (e as { cause?: unknown }).cause
  }
  return false
}

/** The `StaleEpochError` inside `error`'s cause chain, or undefined. */
export function staleEpochOf(error: unknown): StaleEpochError | undefined {
  for (let e: unknown = error, depth = 0; e !== undefined && e !== null && depth < 8; depth++) {
    if (e instanceof StaleEpochError || (e as { name?: unknown }).name === "StaleEpochError") {
      return e as StaleEpochError
    }
    e = (e as { cause?: unknown }).cause
  }
  return undefined
}

/** A handle on one resource's floor, held by the writer that owns `epoch`. */
export interface Fence {
  /** The epoch this writer carries. */
  readonly epoch: number
  /** The refusal this writer has seen so far, if any. Synchronous: for hot paths such as streamed output. */
  readonly stale: StaleEpochError | undefined
  /** Re-reads the floor; throws (and latches `stale`) when it is above `epoch`. */
  check(): Promise<void>
  /** Called once when this writer is first refused. */
  onStale(listener: (error: StaleEpochError) => void): void
  /**
   * Stops all further writes through this fence: every later write is refused
   * with `error`. A driver that must halt for another reason calls this so
   * Pi cannot record the failure it is halting on as a result.
   */
  halt(error: Error): void
}

const FENCE_TABLE = "cua_fence"
/** How often reads re-check the floor (ms). */
const READ_CHECK_MS = 200

/**
 * A `SqliteDatabase` whose every transaction first reads the floor row of
 * table `cua_fence` and refuses when it is above this writer's epoch, and
 * raises it when it is below. The check runs inside the same SQLite
 * transaction (`BEGIN IMMEDIATE`, which holds the file's write lock) as the
 * writes that follow, so a write with a stale epoch can neither slip in
 * between a newer writer's raise and its first commit nor be half applied:
 * the refusal rolls the whole transaction back. Operations outside a
 * transaction go through one, except reads, which are not fenced.
 */
export class FencedSqliteDatabase implements SqliteDatabase, Fence {
  readonly epoch: number
  readonly #inner: SqliteDatabase
  readonly #listeners: Array<(error: StaleEpochError) => void> = []
  #stale: StaleEpochError | undefined
  #halted: Error | undefined
  #lastReadCheck = 0

  private constructor(inner: SqliteDatabase, epoch: number) {
    this.#inner = inner
    this.epoch = epoch
  }

  /** Wraps `inner`, creating the floor row and raising it to `epoch`; refuses when it is already higher. */
  static async open(inner: SqliteDatabase, epoch: number): Promise<FencedSqliteDatabase> {
    assertEpoch(epoch)
    const fenced = new FencedSqliteDatabase(inner, epoch)
    await inner.exec(`CREATE TABLE IF NOT EXISTS ${FENCE_TABLE} (id INTEGER PRIMARY KEY CHECK (id = 1), epoch INTEGER NOT NULL)`)
    // An empty transaction enters the fence: creates or raises the row, or refuses.
    await fenced.transaction(async () => {})
    return fenced
  }

  get stale(): StaleEpochError | undefined {
    return this.#stale
  }

  onStale(listener: (error: StaleEpochError) => void): void {
    this.#listeners.push(listener)
    if (this.#stale) listener(this.#stale)
  }

  #latch(error: StaleEpochError): StaleEpochError {
    if (this.#stale === undefined) {
      this.#stale = error
      for (const listener of this.#listeners) {
        try {
          listener(error)
        } catch {
          // A listener must not mask the refusal.
        }
      }
    }
    return error
  }

  /** The floor now (0 when never set). */
  async floor(): Promise<number> {
    const row = await this.#inner.get<{ epoch: number | bigint }>(`SELECT epoch FROM ${FENCE_TABLE} WHERE id = 1`)
    return row === undefined ? 0 : Number(row.epoch)
  }

  async check(): Promise<void> {
    const floor = await this.floor()
    if (floor > this.epoch) throw this.#latch(new StaleEpochError(this.epoch, floor, "log"))
  }

  halt(error: Error): void {
    this.#halted ??= error
  }

  async #enter(transaction: SqliteExecutor): Promise<void> {
    if (this.#halted) throw this.#halted
    const row = await transaction.get<{ epoch: number | bigint }>(`SELECT epoch FROM ${FENCE_TABLE} WHERE id = 1`)
    if (row === undefined) {
      await transaction.run(`INSERT INTO ${FENCE_TABLE} (id, epoch) VALUES (1, ?)`, this.epoch)
      return
    }
    const floor = Number(row.epoch)
    if (floor > this.epoch) throw new StaleEpochError(this.epoch, floor, "log")
    if (floor < this.epoch) await transaction.run(`UPDATE ${FENCE_TABLE} SET epoch = ? WHERE id = 1`, this.epoch)
  }

  async transaction<T>(callback: (transaction: SqliteExecutor) => Promise<T>): Promise<T> {
    try {
      return await this.#inner.transaction(async (transaction) => {
        await this.#enter(transaction)
        return callback(transaction)
      })
    } catch (error) {
      const stale = staleEpochOf(error)
      if (stale) this.#latch(stale)
      throw error
    }
  }

  exec(sql: string): Promise<void> {
    return this.transaction((transaction) => transaction.exec(sql))
  }

  run(sql: string, ...params: SqliteValue[]): Promise<void> {
    return this.transaction((transaction) => transaction.run(sql, ...params))
  }

  /**
   * Reads are not transactions, but a stale driver that only reads (Pi's scheduler
   * re-reading a task another driver already finished) would spin forever without ever
   * writing, so it would never be refused. Reads therefore look at the floor, at most
   * every `READ_CHECK_MS`, and latch the refusal like a write does.
   */
  async #checkOnRead(): Promise<void> {
    if (this.#stale) throw this.#stale
    const now = Date.now()
    if (now - this.#lastReadCheck < READ_CHECK_MS) return
    this.#lastReadCheck = now
    const floor = await this.floor()
    if (floor > this.epoch) throw this.#latch(new StaleEpochError(this.epoch, floor, "log"))
  }

  async get<T extends object>(sql: string, ...params: SqliteValue[]): Promise<T | undefined> {
    await this.#checkOnRead()
    return this.#inner.get<T>(sql, ...params)
  }

  async all<T extends object>(sql: string, ...params: SqliteValue[]): Promise<T[]> {
    await this.#checkOnRead()
    return this.#inner.all<T>(sql, ...params)
  }

  close(): Promise<void> {
    return this.#inner.close()
  }
}

function assertEpoch(epoch: number): void {
  if (!Number.isSafeInteger(epoch) || epoch < 0) throw new TypeError(`epoch must be a non-negative safe integer, got ${epoch}`)
}

/** A fenced SQLite log: the Pi storage, the database under it and the fence of the writer. */
export interface FencedSqlite {
  storage: SqliteStorage
  database: FencedSqliteDatabase
  fence: Fence
}

/** Opens (or creates) the SQLite log at `path` for the writer of `epoch`, raising the floor and refusing when it is higher. */
export async function openFencedSqlite(
  path: string,
  epoch: number,
  options: NodeSqliteStorageOptions = {},
): Promise<FencedSqlite> {
  // The previous driver may still be writing: SQLite's lock can outlast the busy timeout once.
  let inner: SqliteDatabase | undefined
  let database: FencedSqliteDatabase | undefined
  for (let attempt = 0; database === undefined; attempt++) {
    try {
      inner ??= await openNodeSqliteDatabase(path, options)
      database = await FencedSqliteDatabase.open(inner, epoch)
    } catch (error) {
      if (attempt < 4 && (error as { errcode?: number })?.errcode === 5) {
        await new Promise((resolve) => setTimeout(resolve, 250 * (attempt + 1)))
        continue
      }
      await inner?.close().catch(() => {})
      throw error
    }
  }
  try {
    const storage = await SqliteStorage.open(database)
    return { storage, database, fence: database }
  } catch (error) {
    await database.close().catch(() => {})
    throw error
  }
}

/**
 * A Pi `SqliteStorage` over the SQLite file at `path` whose writes carry
 * `epoch`: every commit is refused with `StaleEpochError` once a newer epoch
 * has opened the same file. Opening raises the floor to `epoch`.
 */
export async function openFencedSqliteStorage(
  path: string,
  epoch: number,
  options: NodeSqliteStorageOptions = {},
): Promise<SqliteStorage> {
  return (await openFencedSqlite(path, epoch, options)).storage
}

const MUTATING = new Set([
  "writeFile",
  "appendFile",
  "truncateFile",
  "flushFile",
  "renameFile",
  "createDir",
  "remove",
  "createTempDir",
  "createTempFile",
])

const MARKER = /^epoch-(\d{20})$/

/**
 * Fences a Pi `FileSystem` (for `JsonlStorage`, or a log on Cua Volume) with
 * epoch marker files in `directory`: raising writes `epoch-<n>` (zero padded)
 * and the floor is the highest marker. Markers are only ever added (lower
 * ones are pruned), so a raise can never be undone by a slower writer, which
 * a single epoch file overwritten in place would allow.
 *
 * Every mutating call first reads the floor and throws `StaleEpochError` when
 * it is above this writer's epoch; appends, writes and renames read it again
 * afterwards and throw if a newer epoch appeared meanwhile.
 *
 * Limits: a filesystem has no transaction spanning the check and the write,
 * so a stale writer that passed the check just before a newer writer raised
 * the floor can still land one write (and then learns about it from the
 * second check). It works across processes only on one filesystem with
 * atomic file creation and directory listing (a local disk, or a volume
 * mount that provides them); it is not a distributed lock. Prefer
 * `openFencedSqliteStorage` when you can.
 */
export class FencedFileSystem implements Fence {
  readonly epoch: number
  readonly fs: FileSystem
  readonly #directory: string
  readonly #listeners: Array<(error: StaleEpochError) => void> = []
  #stale: StaleEpochError | undefined
  #halted: Error | undefined

  halt(error: Error): void {
    this.#halted ??= error
  }

  /** `fs` is the filesystem to wrap, `directory` where markers live (created on `raise()`). */
  constructor(fs: FileSystem, directory: string, epoch: number) {
    assertEpoch(epoch)
    this.fs = fs
    this.#directory = directory
    this.epoch = epoch
  }

  get stale(): StaleEpochError | undefined {
    return this.#stale
  }

  onStale(listener: (error: StaleEpochError) => void): void {
    this.#listeners.push(listener)
    if (this.#stale) listener(this.#stale)
  }

  #latch(error: StaleEpochError): StaleEpochError {
    if (this.#stale === undefined) {
      this.#stale = error
      for (const listener of this.#listeners) {
        try {
          listener(error)
        } catch {
          // A listener must not mask the refusal.
        }
      }
    }
    return error
  }

  /** The floor now (0 when no marker exists). */
  async floor(context: Context = BACKGROUND_CONTEXT): Promise<number> {
    const listing = await this.fs.listDir(this.#directory, context)
    if (!listing.ok) {
      if (listing.error.code === "not_found") return 0
      throw listing.error
    }
    let floor = 0
    for (const entry of listing.value) {
      const match = MARKER.exec(entry.name)
      if (match) floor = Math.max(floor, Number(match[1]))
    }
    return floor
  }

  async check(context: Context = BACKGROUND_CONTEXT): Promise<void> {
    if (this.#halted) throw this.#halted
    const floor = await this.floor(context)
    if (floor > this.epoch) throw this.#latch(new StaleEpochError(this.epoch, floor, `file log ${this.#directory}`))
  }

  /** Raises the floor to this writer's epoch; refuses when it is higher. Call once before the first write. */
  async raise(context: Context = BACKGROUND_CONTEXT): Promise<void> {
    await this.check(context)
    const created = await this.fs.createDir(this.#directory, { recursive: true }, context)
    if (!created.ok) throw created.error
    const name = `epoch-${String(this.epoch).padStart(20, "0")}`
    const path = `${this.#directory}/${name}`
    const written = await this.fs.writeFile(path, `${this.epoch}\n`, context)
    if (!written.ok) throw written.error
    // A newer epoch may have raised the floor while this marker was written.
    await this.check(context)
    // Lower markers are redundant (the floor is the maximum): prune them.
    const listing = await this.fs.listDir(this.#directory, context)
    if (listing.ok) {
      for (const entry of listing.value) {
        const match = MARKER.exec(entry.name)
        if (match && Number(match[1]) < this.epoch) {
          await this.fs.remove(`${this.#directory}/${entry.name}`, { force: true }, context)
        }
      }
    }
  }

  /** The wrapped filesystem: its mutating calls are fenced. */
  fenced(): FileSystem {
    const self = this
    return new Proxy(this.fs, {
      get(target, property) {
        const value = Reflect.get(target, property, target)
        if (typeof value !== "function") return value
        if (typeof property !== "string" || !MUTATING.has(property)) return value.bind(target)
        return async (...args: unknown[]) => {
          const context = (args.at(-1) ?? BACKGROUND_CONTEXT) as Context
          await self.check(context)
          const result = await value.apply(target, args)
          await self.check(context)
          return result
        }
      },
      set(target, property, value) {
        return Reflect.set(target, property, value, target)
      },
    })
  }
}
