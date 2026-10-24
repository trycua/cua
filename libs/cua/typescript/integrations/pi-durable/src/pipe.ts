// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A transparent byte pipe from a local child process to a guest process.
 *
 * Pi's `RemoteExecutionEnv` speaks its framed protocol to a `pi-env` daemon
 * over the stdin and stdout of a local child it spawns (normally `ssh`). This
 * module gives it a child whose stdio is a Unix socket served by this process,
 * which forwards the bytes to `pi-env serve` running in the Space as a
 * cua-spacesd process (stdin through sequenced `SendInput`, stdout from the
 * process stream). Nothing here parses the protocol: Pi's own client and
 * daemon do, so the file system behaves exactly as Pi's conformance suite
 * expects.
 *
 * The guest process carries the environment's fencing epoch, so advancing the
 * epoch (after a restore) kills a stale pipe in the guest.
 */

import { randomUUID } from "node:crypto"
import { mkdtempSync, rmSync } from "node:fs"
import { createServer, type Server, type Socket } from "node:net"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { EVENT_EXIT, EVENT_STDERR, EVENT_STDOUT, type SpacesdLike, killGroup } from "./spacesd.js"

/** Largest stdin chunk sent in one `SendInput`. */
const WRITE_CHUNK = 1024 * 1024

/**
 * The local child: connects to the socket, sends its arguments as the first
 * line, then copies stdin to the socket and the socket to stdout.
 */
const CHILD = `
const net = require("node:net");
const [socketPath, ...args] = process.argv.slice(1);
const socket = net.connect(socketPath, () => {
  socket.write(JSON.stringify(args) + "\\n");
  process.stdin.pipe(socket);
  socket.pipe(process.stdout);
});
socket.on("close", () => process.exit(0));
socket.on("error", () => process.exit(1));
process.stdin.on("end", () => socket.end());
`

export interface GuestPipeOptions {
  /** cua-spacesd of the Space. */
  spacesd: () => Promise<SpacesdLike>
  /** The program to run in the guest (the `pi-env` daemon); the child's arguments are appended. */
  program: () => Promise<string>
  /** Working directory of the guest process. */
  cwd: string
  /** The fencing epoch the guest process is started with (0 = unfenced). */
  epoch: () => number
  /** Diagnostic output (the guest's stderr). */
  onLog?: (text: string) => void
}

/** Serves guest pipes for local children; `command()` is what the child runs. */
export class GuestPipe {
  readonly #options: GuestPipeOptions
  #dir = ""
  #socketPath = ""
  #server: Server | undefined
  #listening: Promise<void> | undefined
  readonly #tags = new Set<string>()
  readonly #sockets = new Set<Socket>()

  constructor(options: GuestPipeOptions) {
    this.#options = options
  }

  /** The local command that bridges to a new guest process (for `ConnectionOptions.command`). */
  async command(): Promise<string[]> {
    await this.#listen()
    return [process.execPath, "-e", CHILD, this.#socketPath]
  }

  #listen(): Promise<void> {
    if (this.#listening) return this.#listening
    this.#dir = mkdtempSync(join(tmpdir(), "cua-pi-pipe-"))
    this.#socketPath = join(this.#dir, "pipe.sock")
    this.#listening = new Promise((resolve, reject) => {
      const server = createServer((socket) => this.#serve(socket))
      server.once("error", reject)
      server.listen(this.#socketPath, () => resolve())
      server.unref()
      this.#server = server
    })
    return this.#listening
  }

  #serve(socket: Socket): void {
    this.#sockets.add(socket)
    let header = Buffer.alloc(0)
    let started = false
    const pending: Buffer[] = []
    let write: ((data: Buffer) => void) | undefined
    socket.on("data", (chunk: Buffer) => {
      if (started) {
        if (write) write(chunk)
        else pending.push(chunk)
        return
      }
      header = Buffer.concat([header, chunk])
      const newline = header.indexOf(0x0a)
      if (newline === -1) return
      started = true
      const args = JSON.parse(header.subarray(0, newline).toString("utf8")) as string[]
      const rest = header.subarray(newline + 1)
      if (rest.length > 0) pending.push(Buffer.from(rest))
      this.#bridge(socket, args, pending, (w) => {
        write = w
      }).catch((error: unknown) => {
        this.#options.onLog?.(`pi-env pipe: ${error instanceof Error ? error.message : String(error)}\n`)
        socket.destroy()
      })
    })
    socket.on("close", () => this.#sockets.delete(socket))
    socket.on("error", () => {})
  }

  async #bridge(
    socket: Socket,
    args: string[],
    pending: Buffer[],
    setWriter: (write: (data: Buffer) => void) => void,
  ): Promise<void> {
    const spacesd = await this.#options.spacesd()
    const program = await this.#options.program()
    const tag = `pi-env:${randomUUID()}`
    const epoch = this.#options.epoch()
    const guest = await spacesd.spawn({
      program,
      args,
      env: new Map(),
      cwd: this.#options.cwd,
      tag,
      stdin: true,
      ...(epoch > 0 ? { epoch: BigInt(epoch) } : {}),
    })
    this.#tags.add(tag)
    // Stdin: one ordered queue of sequenced writes (exactly once across retries).
    let queue: Promise<void> = Promise.resolve()
    const write = (data: Buffer) => {
      for (let offset = 0; offset < data.length; offset += WRITE_CHUNK) {
        const part = data.subarray(offset, offset + WRITE_CHUNK)
        const copy = part.buffer.slice(part.byteOffset, part.byteOffset + part.length) as ArrayBuffer
        queue = queue.then(() => guest.writeStdin(copy)).catch(() => {
          socket.destroy()
        })
      }
    }
    for (const chunk of pending.splice(0)) write(chunk)
    setWriter(write)
    socket.on("close", () => {
      void killGroup(spacesd, tag)
      this.#tags.delete(tag)
    })
    // Stdout to the socket until the guest exits.
    try {
      for (;;) {
        const event = await guest.nextEvent()
        if (event === undefined || event.kind === EVENT_EXIT) break
        if (event.kind === EVENT_STDOUT) {
          if (!socket.write(Buffer.from(event.data))) {
            await new Promise<void>((resolve) => socket.once("drain", resolve))
          }
        } else if (event.kind === EVENT_STDERR) {
          this.#options.onLog?.(Buffer.from(event.data).toString("utf8"))
        }
      }
    } finally {
      this.#tags.delete(tag)
      socket.end()
      void guest.detach().catch(() => {})
    }
  }

  /** Kill every guest process and stop serving. */
  async close(): Promise<void> {
    for (const socket of this.#sockets) socket.destroy()
    if (this.#tags.size > 0) {
      try {
        const spacesd = await this.#options.spacesd()
        await Promise.all([...this.#tags].map((tag) => killGroup(spacesd, tag)))
      } catch {
        // The Space is gone.
      }
    }
    this.#tags.clear()
    this.#server?.close()
    this.#server = undefined
    this.#listening = undefined
    if (this.#dir) rmSync(this.#dir, { recursive: true, force: true })
    this.#dir = ""
  }
}
