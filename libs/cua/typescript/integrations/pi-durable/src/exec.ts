// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * `Shell.exec` on cua-spacesd's `ProcessService`: every command is a guest
 * process started with an optional idempotency key (`request_id`) and fencing
 * epoch, so a retry after a crash reattaches to it or replays its recorded
 * outcome instead of running it again.
 */

import { randomUUID } from "node:crypto"
import type { Context } from "@earendil-works/chord"
import {
  err,
  ExecutionError,
  ok,
  type Result,
  type ShellExecOptions,
  type ShellExecResult,
  StreamDecoder,
} from "@earendil-works/pi-durable/env"
import { StaleEpochError } from "./fence.js"
import {
  EVENT_EXIT,
  EVENT_PTY,
  EVENT_STDERR,
  EVENT_STDOUT,
  type SpacesdCommandLike,
  type SpacesdLike,
  isStaleEpoch,
  killGroup,
} from "./spacesd.js"

const MAX_TIMEOUT_MS = 2_147_483_647

export interface ExecRequest {
  command: string | readonly string[]
  options: ShellExecOptions | undefined
  /** Absolute working directory in the guest. */
  cwd: string
  /** Program and flag that run a string command, e.g. `["bash", "-c"]`. */
  shell: readonly string[]
  /** Idempotency key, or undefined for an unrecorded command. */
  requestId: string | undefined
  /** Fencing epoch (0 = unfenced). */
  epoch: number
  /** Writes the complete output to a temporary file in the guest once a spill threshold is crossed. */
  spillTo: (() => Promise<string>) | undefined
  append: (path: string, bytes: Uint8Array) => Promise<void>
  /** Tags of the commands still running, for `cleanup()`. */
  running: Set<string>
}

/** The argv a request runs, or a spawn error. */
function argv(request: ExecRequest): string[] | ExecutionError {
  const inheritEnv = request.options?.inheritEnv ?? true
  let base: string[]
  if (typeof request.command === "string") {
    base = [...request.shell, request.command]
  } else {
    if (request.command.length === 0) return new ExecutionError("spawn_error", "Empty argv")
    base = [...request.command]
  }
  if (inheritEnv) return base
  // No login environment: `env -i` with only the given variables.
  const vars = Object.entries(request.options?.env ?? {}).map(([k, v]) => `${k}=${v}`)
  return ["env", "-i", ...vars, ...base]
}

/** Runs one command through cua-spacesd. */
export async function execOnSpacesd(
  spacesd: SpacesdLike,
  request: ExecRequest,
  context: Context,
): Promise<Result<ShellExecResult, ExecutionError>> {
  const signal = context.abortSignal
  if (signal?.aborted) return err(new ExecutionError("aborted", "aborted"))
  const options = request.options
  const timeout = options?.timeout
  if (timeout !== undefined) {
    if (!Number.isFinite(timeout) || timeout <= 0) {
      return err(new ExecutionError("timeout", "Invalid timeout: must be a finite number of seconds"))
    }
    if (timeout * 1000 > MAX_TIMEOUT_MS) {
      return err(new ExecutionError("timeout", `Invalid timeout: maximum is ${MAX_TIMEOUT_MS / 1000} seconds`))
    }
  }
  const args = argv(request)
  if (args instanceof ExecutionError) return err(args)
  const [program, ...rest] = args as [string, ...string[]]
  const tag = request.requestId ? `pi:${request.requestId}` : `pi:${randomUUID()}`
  const env = new Map<string, string>()
  if (options?.inheritEnv ?? true) {
    for (const [key, value] of Object.entries(options?.env ?? {})) if (value !== undefined) env.set(key, value)
  }
  const command: SpacesdCommandLike = { program, args: rest, env, cwd: request.cwd, tag, stdin: false }
  if (timeout !== undefined) command.timeoutMs = Math.max(1, Math.round(timeout * 1000))
  if (request.requestId) command.requestId = request.requestId
  if (request.epoch > 0) command.epoch = BigInt(request.epoch)

  let process
  try {
    process = await spacesd.spawn(command)
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    // A refusal by the Space's fence is not a command outcome: a newer driver owns the
    // session, so this one halts instead of recording it (see `StaleEpochError`).
    if (error instanceof StaleEpochError) throw error
    if (isStaleEpoch(error)) throw staleFromMessage(request.epoch, message)
    return err(new ExecutionError("unknown", message))
  }
  request.running.add(tag)
  let aborted = false
  const onAbort = () => {
    aborted = true
    void killGroup(spacesd, tag)
  }
  signal?.addEventListener("abort", onAbort, { once: true })

  const decoders = { stdout: new StreamDecoder(), stderr: new StreamDecoder() }
  let callbackError: ExecutionError | undefined
  const spill = options?.spill
  let bytes = 0
  let lines = 0
  let spillPath: string | undefined
  let spillQueue: Promise<void> = Promise.resolve()
  const held: Uint8Array[] = []
  const deliver = (stream: "stdout" | "stderr", text: string) => {
    if (text === "" || options?.onOutput === undefined || callbackError !== undefined) return
    try {
      options.onOutput(text, context, { stream })
    } catch (error) {
      const cause = error instanceof Error ? error : new Error(String(error))
      callbackError = new ExecutionError("callback_error", cause.message, cause)
      void killGroup(spacesd, tag)
    }
  }
  const record = (chunk: Uint8Array) => {
    if (spill === undefined || request.spillTo === undefined) return
    bytes += chunk.length
    for (const byte of chunk) if (byte === 0x0a) lines++
    if (spillPath === undefined) {
      held.push(chunk)
      if (bytes > spill.afterBytes || lines > spill.afterLines) {
        const pending = held.splice(0)
        const spillTo = request.spillTo
        spillQueue = spillQueue.then(async () => {
          spillPath = await spillTo()
          for (const part of pending) await request.append(spillPath, part)
        })
        spillPath = "" // set by the queue; marks that spilling started
      }
    } else {
      spillQueue = spillQueue.then(() => request.append(spillPath!, chunk))
    }
  }

  try {
    for (;;) {
      const event = await process.nextEvent()
      if (event === undefined) break
      if (event.kind === EVENT_EXIT) {
        deliver("stdout", decoders.stdout.decode())
        deliver("stderr", decoders.stderr.decode())
        try {
          await spillQueue
        } catch {
          spillPath = undefined
        }
        const exit = event.exit
        const spilled = spillPath ? { spillPath } : {}
        if (callbackError) return err(callbackError)
        if (aborted) {
          const failure = new ExecutionError("aborted", "aborted")
          if (spillPath) failure.spillPath = spillPath
          return err(failure)
        }
        if (exit?.timedOut) {
          const failure = new ExecutionError("timeout", `timeout:${timeout}`)
          if (spillPath) failure.spillPath = spillPath
          return err(failure)
        }
        if (exit?.code === undefined || exit.code === null) {
          const message = exit?.error ?? (exit?.signal ? `killed by ${exit.signal}` : "no exit status")
          if (exit?.error && /not found|failed to start|does not exist/.test(exit.error)) {
            return err(new ExecutionError("spawn_error", exit.error))
          }
          if (exit?.signal && !exit.error) {
            // Killed by a signal: report it the way a shell does, 128 + n.
            return ok({ exitCode: 128 + signalNumber(exit.signal), ...spilled })
          }
          return err(new ExecutionError("unknown", message))
        }
        return ok({ exitCode: exit.code, ...spilled })
      }
      const data = new Uint8Array(event.data)
      record(data)
      if (event.kind === EVENT_STDERR) deliver("stderr", decoders.stderr.decode(data))
      else if (event.kind === EVENT_STDOUT || event.kind === EVENT_PTY) deliver("stdout", decoders.stdout.decode(data))
    }
    return err(new ExecutionError("unknown", "the command's output ended without an exit status"))
  } catch (error) {
    if (callbackError) return err(callbackError)
    if (aborted) return err(new ExecutionError("aborted", "aborted"))
    return err(new ExecutionError("unknown", error instanceof Error ? error.message : String(error)))
  } finally {
    signal?.removeEventListener("abort", onAbort)
    request.running.delete(tag)
    void process.detach().catch(() => {})
  }
}

/** A `StaleEpochError` for cua-spacesd's "epoch N is older than the Space's current epoch M" refusal. */
export function staleFromMessage(epoch: number, message: string): StaleEpochError {
  const floor = /current epoch (\d+)/i.exec(message)?.[1]
  return new StaleEpochError(epoch, floor === undefined ? 0 : Number(floor), "space", { cause: new Error(message) })
}

const SIGNALS: Record<string, number> = { hup: 1, int: 2, quit: 3, kill: 9, usr1: 10, usr2: 12, term: 15 }

function signalNumber(name: string): number {
  return SIGNALS[name.toLowerCase().replace(/^sig(nal_)?/, "")] ?? 0
}
