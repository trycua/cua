// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The slice of the cua SDK's `SpacesdClient` (`@trycua/cua`) this package
 * uses, typed structurally so tests can stand in a fake and the adapter never
 * reaches past the SDK into the wire protocol.
 */

/** `ProcessEventKind` of `@trycua/cua`: stdout, stderr, PTY, exit (in that order, from 0). */
export const EVENT_STDOUT = 0
export const EVENT_STDERR = 1
export const EVENT_PTY = 2
export const EVENT_EXIT = 3

export interface ExitInfoLike {
  code?: number
  signal?: string
  timedOut: boolean
  error?: string
  success: boolean
}

export interface ProcessEventLike {
  kind: number
  offset: bigint
  data: ArrayBuffer
  exit?: ExitInfoLike
}

export interface SpacesdProcessLike {
  pid(): number
  nextEvent(): Promise<ProcessEventLike | undefined>
  writeStdin(data: ArrayBuffer): Promise<void>
  closeStdin(): Promise<void>
  detach(): Promise<void>
}

/** `SpacesdCommand` of `@trycua/cua`. */
export interface SpacesdCommandLike {
  program: string
  args: string[]
  env: Map<string, string>
  cwd?: string
  timeoutMs?: number
  tag?: string
  stdin: boolean
  requestId?: string
  epoch?: bigint
}

export interface SpacesdLike {
  spawn(command: SpacesdCommandLike): Promise<SpacesdProcessLike>
  /** A unary `cua.env.v1` RPC with proto3-JSON in and out. */
  callJson(method: string, requestJson: string): Promise<string>
  upload(
    path: string,
    data: ArrayBuffer,
    options: { createNew: boolean; append: boolean; permissions: number; createParents: boolean } | undefined,
  ): Promise<unknown>
  download(path: string): Promise<ArrayBuffer>
}

/** What the guest's exec ledger knows about one request id (`ExecRecord`). */
export interface ExecRecord {
  state: "running" | "exited" | "lost"
  epoch: number
  exitCode?: number
  signal?: string
  timedOut: boolean
  error?: string
  outputBytes: number
  outputSha256?: string
}

const STATES: Record<string, ExecRecord["state"]> = {
  EXEC_STATE_RUNNING: "running",
  EXEC_STATE_EXITED: "exited",
  EXEC_STATE_LOST: "lost",
}

interface RecordJson {
  state?: string
  epoch?: string | number
  end?: { exitCode?: number; signal?: string; timedOut?: boolean; error?: string; lost?: boolean }
  outputBytes?: string | number
  outputSha256?: string
}

/** `ProcessService.GetExecRecord`: the record of `requestId` and the Space's current epoch. */
export async function getExecRecord(
  spacesd: SpacesdLike,
  requestId: string,
): Promise<{ record: ExecRecord | undefined; epoch: number }> {
  const raw = JSON.parse(
    await spacesd.callJson("ProcessService/GetExecRecord", JSON.stringify({ requestId })),
  ) as { record?: RecordJson; epoch?: string | number }
  const epoch = Number(raw.epoch ?? 0)
  const r = raw.record
  const state = r?.state ? STATES[r.state] : undefined
  if (!r || !state) return { record: undefined, epoch }
  const end = r.end ?? {}
  const record: ExecRecord = {
    state,
    epoch: Number(r.epoch ?? 0),
    timedOut: end.timedOut ?? false,
    outputBytes: Number(r.outputBytes ?? 0),
  }
  if (end.exitCode !== undefined) record.exitCode = end.exitCode
  if (end.signal !== undefined && end.signal !== "SIGNAL_UNSPECIFIED") record.signal = end.signal
  if (end.error) record.error = end.error
  if (r.outputSha256) record.outputSha256 = r.outputSha256
  return { record, epoch }
}

/** `ProcessService.AdvanceEpoch`: raises the fencing epoch, killing older fenced processes. */
export async function advanceEpoch(
  spacesd: SpacesdLike,
  epoch: number,
): Promise<{ epoch: number; killed: number }> {
  const raw = JSON.parse(
    await spacesd.callJson("ProcessService/AdvanceEpoch", JSON.stringify({ epoch: String(epoch) })),
  ) as { epoch?: string | number; killed?: number }
  return { epoch: Number(raw.epoch ?? 0), killed: raw.killed ?? 0 }
}

/** SIGKILL to a managed process's whole group, by tag. Never throws. */
export async function killGroup(spacesd: SpacesdLike, tag: string): Promise<void> {
  try {
    await spacesd.callJson(
      "ProcessService/SignalProcess",
      JSON.stringify({ process: { tag }, signal: "SIGNAL_KILL", processGroup: true }),
    )
  } catch {
    // Already gone.
  }
}

/** Whether `error` is the daemon's refusal of a stale fencing epoch. */
export function isStaleEpoch(error: unknown): boolean {
  const text = String((error as { message?: unknown })?.message ?? error)
  return /older than the Space's current epoch|STALE_EPOCH|StaleEpoch/i.test(text)
}

/** Runs `script` with `/bin/sh -c` to completion (unfenced, no ledger): stdout, stderr and the exit code. */
export async function sh(
  spacesd: SpacesdLike,
  script: string,
  options: { epoch?: number; timeoutMs?: number } = {},
): Promise<{ code: number | undefined; stdout: string; stderr: string; error?: string }> {
  const command: SpacesdCommandLike = {
    program: "/bin/sh",
    args: ["-c", script],
    env: new Map(),
    stdin: false,
  }
  if (options.epoch) command.epoch = BigInt(options.epoch)
  if (options.timeoutMs !== undefined) command.timeoutMs = options.timeoutMs
  const process = await spacesd.spawn(command)
  const out: Buffer[] = []
  const err: Buffer[] = []
  for (;;) {
    const event = await process.nextEvent()
    if (event === undefined) return { code: undefined, stdout: "", stderr: "", error: "no exit" }
    if (event.kind === EVENT_STDOUT || event.kind === EVENT_PTY) out.push(Buffer.from(event.data))
    else if (event.kind === EVENT_STDERR) err.push(Buffer.from(event.data))
    else if (event.kind === EVENT_EXIT) {
      const result: { code: number | undefined; stdout: string; stderr: string; error?: string } = {
        code: event.exit?.code,
        stdout: Buffer.concat(out).toString("utf8"),
        stderr: Buffer.concat(err).toString("utf8"),
      }
      if (event.exit?.error) result.error = event.exit.error
      return result
    }
  }
}

/** A POSIX shell word. */
export function quote(text: string): string {
  return `'${text.replaceAll("'", "'\\''")}'`
}
