// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * `SpacesExecutionEnv`: Pi Durable's `ExecutionEnv` on a Cua Space.
 *
 * - Commands (`Shell`) run as cua-spacesd processes. A command may carry an
 *   idempotency key and the Space's fencing epoch; `withRequestIds()` returns
 *   a view that stamps a stable key on every command, which is what makes a
 *   retried tool call adopt the first run's outcome instead of running again.
 * - Files (`FileSystem`) go through Pi's own `RemoteExecutionEnv` client to a
 *   `pi-env` daemon in the Space, reached over a transparent pipe through
 *   cua-spacesd (see `pipe.ts`), so file semantics are exactly Pi's.
 */

import { createHash } from "node:crypto"
import { readFileSync } from "node:fs"
import type { Context } from "@earendil-works/chord"
import type {
  BinaryReader,
  DirReader,
  ExecutionEnv,
  ExecutionError,
  FileError,
  FileInfo,
  FileWatcher,
  Result,
  ShellExecOptions,
  ShellExecResult,
  TextLineReader,
  WatchChange,
  WatchTarget,
} from "@earendil-works/pi-durable/env"
import { Connection, packagedDaemon, RemoteExecutionEnv } from "@earendil-works/pi-env"
import { execOnSpacesd, staleFromMessage } from "./exec.js"
import { type StaleEpochError, isStaleEpochError } from "./fence.js"
import { GuestPipe } from "./pipe.js"
import { type SpacesdLike, isStaleEpoch, quote, sh } from "./spacesd.js"

export interface SpacesExecutionEnvOptions {
  /** cua-spacesd of the Space (`(await sandbox.spacesd()).` from `@trycua/cua`), or a function that connects. */
  spacesd: SpacesdLike | (() => Promise<SpacesdLike>)
  /** The file namespace (`FileSystem.id`), e.g. `cua-space:<name>`. */
  id: string
  /** Absolute working directory in the Space. */
  cwd: string
  /** Program and flag that run a string command. Default `["bash", "-c"]`. */
  shell?: readonly string[]
  /** The fencing epoch commands carry (0 or absent: unfenced). Read at every command. */
  epoch?: () => number
  /** Guest path of a `pi-env` daemon. Default: the one packaged with `@earendil-works/pi-env`, uploaded once. */
  piEnvPath?: string
  /** Diagnostic output of the file daemon and its pipe. */
  onLog?: (text: string) => void
  /**
   * Called when the Space refuses an operation of this environment as older
   * than its current epoch (a newer driver owns the session). The refusal is
   * also thrown from `exec` as a `StaleEpochError`; file operations, whose
   * errors Pi turns into results, rely on this to learn of it.
   */
  onStale?: (error: StaleEpochError) => void
}

const ARCH: Record<string, "x64" | "arm64"> = { x86_64: "x64", amd64: "x64", aarch64: "arm64", arm64: "arm64" }

export class SpacesExecutionEnv implements ExecutionEnv {
  readonly id: string
  readonly #options: SpacesExecutionEnvOptions
  readonly #files: RemoteExecutionEnv
  readonly #pipe: GuestPipe
  readonly #running = new Set<string>()
  #spacesd: Promise<SpacesdLike> | undefined
  #daemon: Promise<string> | undefined

  constructor(options: SpacesExecutionEnvOptions) {
    this.id = options.id
    this.#options = options
    this.#pipe = new GuestPipe({
      spacesd: () => this.spacesd(),
      program: () => this.#daemonPath(),
      cwd: options.cwd,
      epoch: () => this.epoch(),
      ...(options.onLog ? { onLog: options.onLog } : {}),
    })
    const connection = new Connection({
      command: () => this.#pipe.command(),
      ...(options.onLog ? { onLog: options.onLog } : {}),
    })
    this.#files = new RemoteExecutionEnv({ connection, id: options.id, cwd: options.cwd })
  }

  get cwd(): string {
    return this.#files.cwd
  }

  set cwd(value: string) {
    this.#files.cwd = value
  }

  /** The Space's cua-spacesd connection. */
  spacesd(): Promise<SpacesdLike> {
    if (this.#spacesd === undefined) {
      const source = this.#options.spacesd
      const connecting = (typeof source === "function" ? source() : Promise.resolve(source)).then((client) =>
        this.#watchStale(client),
      )
      this.#spacesd = connecting
      connecting.catch(() => {
        if (this.#spacesd === connecting) this.#spacesd = undefined
      })
    }
    return this.#spacesd
  }

  /** `client` with spawn refusals by the Space's epoch fence surfaced as `StaleEpochError` (and reported). */
  #watchStale(client: SpacesdLike): SpacesdLike {
    const onStale = this.#options.onStale
    return new Proxy(client, {
      get: (target, property) => {
        const value = Reflect.get(target, property, target)
        if (property !== "spawn" || typeof value !== "function") return typeof value === "function" ? value.bind(target) : value
        return async (command: Parameters<SpacesdLike["spawn"]>[0]) => {
          try {
            return await value.call(target, command)
          } catch (error) {
            if (isStaleEpoch(error) || isStaleEpochError(error)) {
              const stale = isStaleEpochError(error)
                ? (error as StaleEpochError)
                : staleFromMessage(Number(command.epoch ?? 0), String((error as Error)?.message ?? error))
              onStale?.(stale)
              throw stale
            }
            throw error
          }
        }
      },
    })
  }

  /** The fencing epoch commands carry now. */
  epoch(): number {
    return this.#options.epoch?.() ?? 0
  }

  /** The guest path of `pi-env`, uploading the packaged daemon (named by its SHA-256) when absent. */
  #daemonPath(): Promise<string> {
    if (this.#options.piEnvPath) return Promise.resolve(this.#options.piEnvPath)
    if (this.#daemon === undefined) {
      const deploying = this.#deploy()
      this.#daemon = deploying
      deploying.catch(() => {
        if (this.#daemon === deploying) this.#daemon = undefined
      })
    }
    return this.#daemon
  }

  async #deploy(): Promise<string> {
    const spacesd = await this.spacesd()
    const probe = await sh(spacesd, "uname -s; uname -m")
    const [system, machine] = probe.stdout.trim().split("\n")
    const arch = ARCH[(machine ?? "").trim()]
    if (system?.trim() !== "Linux" || arch === undefined) {
      throw new Error(`pi-env: unsupported guest ${probe.stdout.trim() || probe.error}`)
    }
    const local = packagedDaemon({ platform: "linux", arch })
    const bytes = readFileSync(local)
    const sha = createHash("sha256").update(bytes).digest("hex")
    const path = `/var/tmp/cua-pi-env/pi-env-${sha.slice(0, 32)}`
    const present = await sh(spacesd, `test -x ${quote(path)} && sha256sum ${quote(path)}`)
    if (present.code === 0 && present.stdout.startsWith(sha)) return path
    await sh(spacesd, "mkdir -p /var/tmp/cua-pi-env && chmod 1777 /var/tmp/cua-pi-env 2>/dev/null; true")
    const staging = `${path}.${process.pid}.tmp`
    const copy = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.length) as ArrayBuffer
    await spacesd.upload(staging, copy, { createNew: false, append: false, permissions: 0o755, createParents: true })
    const moved = await sh(
      spacesd,
      `chmod 755 ${quote(staging)} && echo "${sha}  ${staging}" | sha256sum -c - >/dev/null && mv -f ${quote(staging)} ${quote(path)}`,
    )
    if (moved.code !== 0) throw new Error(`pi-env: deploying to the Space failed: ${moved.stderr || moved.error}`)
    return path
  }

  /**
   * A view of this environment whose commands carry stable idempotency keys
   * `<prefix>:<n>@<epoch>` (n counts the view's commands), so a retry of the
   * same work after a crash reattaches to, or replays, the first run.
   */
  withRequestIds(prefix: string, keyEpoch?: number): ExecutionEnv {
    let next = 0
    const exec = (
      command: string | readonly string[],
      options: ShellExecOptions | undefined,
      context: Context,
    ): Promise<Result<ShellExecResult, ExecutionError>> =>
      this.#exec(command, options, context, `${prefix}:${next++}@${keyEpoch ?? this.epoch()}`)
    // The real object is the receiver: private fields do not resolve on a proxy.
    return new Proxy(this, {
      get(target, property) {
        if (property === "exec") return exec
        const value = Reflect.get(target, property, target)
        return typeof value === "function" ? value.bind(target) : value
      },
      set(target, property, value) {
        return Reflect.set(target, property, value, target)
      },
    }) as ExecutionEnv
  }

  exec(
    command: string | readonly string[],
    options: ShellExecOptions | undefined,
    context: Context,
  ): Promise<Result<ShellExecResult, ExecutionError>> {
    return this.#exec(command, options, context, undefined)
  }

  async #exec(
    command: string | readonly string[],
    options: ShellExecOptions | undefined,
    context: Context,
    requestId: string | undefined,
  ): Promise<Result<ShellExecResult, ExecutionError>> {
    let cwd = this.cwd
    if (options?.cwd) {
      const resolved = await this.#files.absolutePath(options.cwd, context)
      if (resolved.ok) cwd = resolved.value
    }
    const spacesd = await this.spacesd()
    return execOnSpacesd(
      spacesd,
      {
        command,
        options,
        cwd,
        shell: this.#options.shell ?? ["bash", "-c"],
        requestId,
        epoch: this.epoch(),
        spillTo: async () => {
          const file = await this.#files.createTempFile({ prefix: "pi-output-", suffix: ".log" }, context)
          if (!file.ok) throw file.error
          return file.value
        },
        append: async (path, bytes) => {
          const appended = await this.#files.appendFile(path, bytes, context)
          if (!appended.ok) throw appended.error
        },
        running: this.#running,
      },
      context,
    )
  }

  async cleanup(context: Context): Promise<void> {
    const tags = [...this.#running]
    this.#running.clear()
    if (tags.length > 0) {
      try {
        const spacesd = await this.spacesd()
        await Promise.all(
          tags.map((tag) =>
            spacesd
              .callJson(
                "ProcessService/SignalProcess",
                JSON.stringify({ process: { tag }, signal: "SIGNAL_KILL", processGroup: true }),
              )
              .catch(() => ""),
          ),
        )
      } catch {
        // The Space is gone.
      }
    }
    await this.#files.cleanup(context)
  }

  /** Stop the file daemon and its pipe. The environment reconnects on next use. */
  async close(): Promise<void> {
    await this.#pipe.close()
  }

  /**
   * Drop the connection to the Space (for example after it was replaced by a
   * restore); the next operation connects again.
   */
  async reconnect(): Promise<void> {
    this.#spacesd = undefined
    await this.#pipe.close()
  }

  // FileSystem: Pi's client over the pipe.

  absolutePath(path: string, context: Context): Promise<Result<string, FileError>> {
    return this.#files.absolutePath(path, context)
  }
  joinPath(parts: string[], context: Context): Promise<Result<string, FileError>> {
    return this.#files.joinPath(parts, context)
  }
  readTextFile(path: string, context: Context): Promise<Result<string, FileError>> {
    return this.#files.readTextFile(path, context)
  }
  openTextLineReader(path: string, context: Context): Promise<Result<TextLineReader, FileError>> {
    return this.#files.openTextLineReader(path, context)
  }
  readTextLines(
    path: string,
    options: { maxLines?: number } | undefined,
    context: Context,
  ): Promise<Result<string[], FileError>> {
    return this.#files.readTextLines(path, options, context)
  }
  readBinaryFile(path: string, context: Context): Promise<Result<Uint8Array, FileError>> {
    return this.#files.readBinaryFile(path, context)
  }
  openBinaryReader(
    path: string,
    options: { noFollow?: boolean } | undefined,
    context: Context,
  ): Promise<Result<BinaryReader, FileError>> {
    return this.#files.openBinaryReader(path, options, context)
  }
  writeFile(path: string, content: string | Uint8Array, context: Context): Promise<Result<void, FileError>> {
    return this.#files.writeFile(path, content, context)
  }
  appendFile(path: string, content: string | Uint8Array, context: Context): Promise<Result<void, FileError>> {
    return this.#files.appendFile(path, content, context)
  }
  truncateFile(path: string, size: number, context: Context): Promise<Result<void, FileError>> {
    return this.#files.truncateFile(path, size, context)
  }
  flushFile(path: string, context: Context): Promise<Result<void, FileError>> {
    return this.#files.flushFile(path, context)
  }
  renameFile(sourcePath: string, destinationPath: string, context: Context): Promise<Result<void, FileError>> {
    return this.#files.renameFile(sourcePath, destinationPath, context)
  }
  fileInfo(path: string, context: Context): Promise<Result<FileInfo, FileError>> {
    return this.#files.fileInfo(path, context)
  }
  listDir(path: string, context: Context): Promise<Result<FileInfo[], FileError>> {
    return this.#files.listDir(path, context)
  }
  openDirReader(path: string, context: Context): Promise<Result<DirReader, FileError>> {
    return this.#files.openDirReader(path, context)
  }
  watch(
    targets: readonly WatchTarget[],
    onChange: (change: WatchChange) => void,
    context: Context,
  ): Promise<Result<FileWatcher, FileError>> {
    return this.#files.watch(targets, onChange, context)
  }
  canonicalPath(path: string, context: Context): Promise<Result<string, FileError>> {
    return this.#files.canonicalPath(path, context)
  }
  exists(path: string, context: Context): Promise<Result<boolean, FileError>> {
    return this.#files.exists(path, context)
  }
  createDir(
    path: string,
    options: { recursive?: boolean } | undefined,
    context: Context,
  ): Promise<Result<void, FileError>> {
    return this.#files.createDir(path, options, context)
  }
  remove(
    path: string,
    options: { recursive?: boolean; force?: boolean } | undefined,
    context: Context,
  ): Promise<Result<void, FileError>> {
    return this.#files.remove(path, options, context)
  }
  createTempDir(prefix: string | undefined, context: Context): Promise<Result<string, FileError>> {
    return this.#files.createTempDir(prefix, context)
  }
  createTempFile(
    options: { prefix?: string; suffix?: string } | undefined,
    context: Context,
  ): Promise<Result<string, FileError>> {
    return this.#files.createTempFile(options, context)
  }
}
