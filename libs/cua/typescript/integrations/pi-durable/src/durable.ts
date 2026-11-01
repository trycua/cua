// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Keeps a Space consistent with a Pi Durable conversation log across crashes.
 *
 * Pi Durable records a tool call's intent before running it and its result
 * entry after. A crash in between leaves the call's effect on the sandbox
 * unknown; by default Pi then reports the call as interrupted and the sandbox
 * keeps whatever partial state the crash left. `durableSpaces()` closes that
 * gap for the tools it wraps (by default `bash`, `write` and `edit`):
 *
 * - Mutating calls run one at a time per Space. After a call's effects and
 *   before its result entry is committed, the Space is checkpointed and the
 *   checkpoint recorded in the conversation's `cua.space` document, keyed by
 *   the call (and, once known, its result entry). The state before a call is
 *   therefore always the newest recorded checkpoint.
 * - Every command carries an idempotency key and the Space's fencing epoch.
 *   cua-spacesd records each key in its exec ledger before the command starts.
 * - When Pi recovers an interrupted call, the wrapper asks the ledger: an
 *   exited command's recorded outcome is adopted, a running one is reattached,
 *   and a command that was never started simply runs. Only when the outcome
 *   is unknown (the command was lost with its daemon, or a file tool was cut
 *   off mid-write) is the Space restored to the checkpoint before the call,
 *   with a new epoch that fences anything still running from the old one.
 *   The call then reruns (tools listed in `rerun`) or ends with an
 *   `interrupted` result that says the sandbox was restored.
 * - A forked conversation gets its own Space, forked from the checkpoint its
 *   parent recorded at or before the fork entry.
 *
 * That is checkpoint mode. Transactional mode (see `mode`) replaces the
 * per-call checkpoint with a diff of the workspace committed in the same log
 * transaction as the call's outcome: the log holds a content-addressed image
 * of the workspace, recovery makes the Space equal to it, and nothing is
 * checkpointed per call. `mode: "auto"` (the default) picks transactional mode
 * for small workspaces and falls back to checkpoint mode, permanently, when a
 * limit is crossed.
 *
 * Both modes are fenced by the session epoch (`epoch`, or `CUA_SESSION_EPOCH`):
 * commands carry at least that epoch, the Space is advanced to it when first
 * used, and a refusal by the Space or the log as stale halts the driver with
 * `StaleEpochError` instead of becoming a tool result.
 */

import { createHash, randomUUID } from "node:crypto"
import type { Context, JsonValue } from "@earendil-works/chord"
import {
  type ConversationId,
  defineDoc,
  defineExtension,
  type EnvTarget,
  type Extension,
  type HarnessOptions,
  type ToolExecutionApi,
  type ToolExecutionResult,
  type ToolRegistration,
  wrapTool,
} from "@earendil-works/pi-durable"
import type { ExecutionEnv } from "@earendil-works/pi-durable/env"
import { SpacesExecutionEnv, type SpacesExecutionEnvOptions } from "./env.js"
import { staleFromMessage } from "./exec.js"
import { type Fence, HaltError, haltOf, type StaleEpochError, staleEpochOf } from "./fence.js"
import { checkpointLabel, type SnapshotProvider, type SpaceProvider } from "./providers.js"
import { advanceEpoch, getExecRecord, isStaleEpoch, quote, sh } from "./spacesd.js"
import {
  applyDiff,
  backgroundProcesses,
  capture,
  DEFAULT_TRANSACTIONAL_LIMITS,
  diff,
  type ImageState,
  reconcile,
  scan,
  type TransactionalLimits,
  TransactionalLimitExceeded,
  TransactionalUnsupported,
} from "./transactional.js"

/** One recorded checkpoint of a conversation's Space. */
export type CheckpointRecord = {
  /** Checkpoint id (`SnapshotProvider`). */
  id: string
  /** The tool call whose effects it captures; absent for the base state. */
  callId?: string
  /** The tool task of that call. */
  taskId?: number
  /** That call's result entry, filled in once committed. */
  entryId?: number
}

/** The `cua.space` document: which Space a conversation uses and its recorded checkpoints. */
export type SpaceState = {
  owner?: number
  space?: string
  epoch?: number
  checkpoints?: CheckpointRecord[]
  /** A restore in progress: redone on recovery before anything else. */
  restoring?: { to: string; epoch: number }
  /** Set once this conversation has fallen back to checkpoint mode for good, and why. */
  mode?: "checkpoint"
  modeReason?: string
}

/**
 * Rewindable and forked as of the fork entry, so a fork sees exactly the
 * checkpoints its parent had recorded at that entry.
 */
export const SpaceDoc = defineDoc<SpaceState>({
  kind: "cua.space",
  version: 1,
  scope: "conversation",
  history: "rewindable",
  fork: "asOf",
  initial: () => ({}),
})

/**
 * Transactional mode's image of the workspace: rewindable and forked as of
 * the fork entry, like `SpaceDoc`, so a fork rebuilds its own Space from the
 * workspace its parent had at that entry.
 */
export const ImageDoc = defineDoc<ImageState>({
  kind: "cua.space.image",
  version: 1,
  scope: "conversation",
  history: "rewindable",
  fork: "asOf",
  initial: () => ({}),
})

/** Per tool call: whether effects started, and the outcome once known. Retired with the task. */
type CallState = {
  /** Effects may have started, under this epoch, in this incarnation of the Space. */
  started?: {
    epoch: number
    incarnation?: string
    /** Transactional mode: the Space must be made equal to the image before this call runs (again). */
    undo?: { rerun: boolean }
    /** The epoch of the call's idempotency keys when it differs from `epoch` (the session epoch moved on after the call started). */
    keyEpoch?: number
  }
  /** The tool's result, its streamed output (tail) and last details, or the error it threw. */
  outcome?: {
    result?: JsonValue
    output?: string
    dropped?: Skip
    details?: JsonValue
    thrown?: string
    incarnation?: string
  }
  /** This call's outcome was unknown, so the Space was restored to `to` under `epoch`; `rerun` says what follows. */
  restored?: { to: CheckpointRecord; epoch: number; rerun: boolean }
}

type Skip = { bytes: number; newlines: number; endsWithNewline: boolean }

/** Streamed output kept to replay an adopted outcome; the tool keeps at most a tail of this. */
const MAX_CAPTURE = 1024 * 1024

const CallDoc = defineDoc<CallState>({
  kind: "cua.space.call",
  version: 1,
  scope: "task",
  initial: () => ({}),
})

export interface DurableSpacesOptions {
  spaces: SpaceProvider
  snapshots: SnapshotProvider
  /** Working directory in each Space (and the default agent `cwd`). */
  cwd: string
  /** The Space of a conversation. Default `pi-<hash of the conversation id and namespace>`. */
  spaceName?: (conversationId: ConversationId) => string
  /** Separates Space names of different Harness storages. Default `"default"`. */
  namespace?: string
  /** Tools to wrap. Default `["bash", "write", "edit"]`. */
  tools?: readonly string[]
  /**
   * Tools that rerun after a restore when their outcome is unknown. Default
   * `["write", "edit"]`: given the restored state they are deterministic. A
   * tool whose own `replay` is `"safe"` also reruns. Others end with an
   * `interrupted` result.
   */
  rerun?: readonly string[]
  /** Passed to each `SpacesExecutionEnv`. */
  env?: Pick<SpacesExecutionEnvOptions, "shell" | "piEnvPath" | "onLog">
  /**
   * How a Space is kept consistent with the log. Default `"auto"`.
   * - `"checkpoint"`: a Space (or workspace) checkpoint per call.
   * - `"transactional"`: the call's workspace diff is committed with its
   *   outcome; no per-call checkpoint. Refuses a workspace over `transactional`'s
   *   limits instead of falling back.
   * - `"auto"`: transactional while the Space is container-like, the workspace
   *   is within the limits and no wrapped command leaves processes running;
   *   otherwise checkpoint, for good from that call on. A conversation that
   *   already has checkpoints keeps using them.
   */
  mode?: "auto" | "transactional" | "checkpoint"
  /** Limits of transactional mode: files (1000), bytes (16 MiB) and paths left out of the image. */
  transactional?: Partial<TransactionalLimits>
  /**
   * The session epoch, a number that only grows (the coordinator issues one
   * per drive attempt). Default: `CUA_SESSION_EPOCH`. Commands carry at least
   * this epoch and a Space is advanced to it when first used, so any older
   * driver is cut off.
   */
  epoch?: number
  /**
   * The fence of the log this driver writes to (`openFencedSqlite().fence`).
   * It is checked before each call and before live output is forwarded; once
   * it reports a newer epoch the call halts with `StaleEpochError`.
   */
  fence?: Fence
  /**
   * Called once when the Space or the log refuses this driver as stale (a newer
   * epoch owns the session). The error is also thrown up through the tool; Pi
   * may still try to record it, which the fenced log then refuses, so a driver
   * should stop on this callback (see `runDurableSession`).
   */
  onStale?: (error: StaleEpochError) => void
  /**
   * Called once when the driver must halt for a reason other than staleness
   * (see `HaltError`). The error is also thrown; Pi may record that throw as a
   * tool result unless the log's fence is halted too, which `runDurableSession`
   * does from this callback.
   */
  onHalt?: (error: HaltError) => void
  /** Where a Space records which life it is in. Default `/var/tmp/cua-pi-durable/incarnation`. */
  incarnationMarker?: string
  /** Observes recovery decisions (for logs and tests). */
  onRecovery?: (event: RecoveryEvent) => void
  /**
   * Called at each step of a wrapped call (`call`, `started`, `outcome`,
   * `checkpointed`, `recorded`), for logs and fault-injection tests.
   */
  trace?: (point: string, callId: string) => void
}

export type RecoveryEvent = (
  | { kind: "adopted"; conversationId: number; callId: string; via: "result" | "ledger" }
  | { kind: "reattached"; conversationId: number; callId: string }
  | { kind: "restored"; conversationId: number; callId: string; checkpoint: string; epoch: number }
  | { kind: "forked"; conversationId: number; space: string; from: string }
  | { kind: "rebuilt"; conversationId: number; space: string; from: string }
  /** Transactional mode: the Space was made equal to the log's image (`files` written or deleted) under `epoch`. */
  | { kind: "reverted"; conversationId: number; callId: string; files: number; epoch: number }
  /** Transactional mode: the Space was rebuilt from the log's image (lost, replaced or forked). */
  | { kind: "reimaged"; conversationId: number; callId: string; files: number }
  /** The conversation fell back to checkpoint mode, for good. */
  | { kind: "mode"; conversationId: number; callId: string; to: "checkpoint"; reason: string }
) & {
  /** The mode the decision was taken in. */
  mode: "checkpoint" | "transactional"
}

/** One Space as used by one process: its environment and a lock that serializes mutating calls. */
class Binding {
  readonly name: string
  readonly env: SpacesExecutionEnv
  epoch: number
  /**
   * Which life of the Space this is: it changes when a lost Space is rebuilt
   * from a stored checkpoint, so effects recorded in an earlier life are
   * known to be gone.
   */
  readonly incarnation: string
  /** A refusal of this Space's epoch fence seen by its environment: a newer driver owns the session. */
  stale: StaleEpochError | undefined
  /** The Space's kind (`container`, `microvm`, ...), once asked. */
  kind: Promise<string | undefined> | undefined
  #tail: Promise<unknown> = Promise.resolve()

  constructor(
    name: string,
    epoch: number,
    incarnation: string,
    make: (binding: Binding) => SpacesExecutionEnv,
  ) {
    this.name = name
    this.epoch = epoch
    this.incarnation = incarnation
    this.env = make(this)
  }

  /** Run `work` after every earlier call on this Space. */
  serialize<T>(work: () => Promise<T>): Promise<T> {
    const run = this.#tail.then(work, work)
    this.#tail = run.catch(() => {})
    return run
  }
}

export interface DurableSpaces {
  /** For `HarnessOptions.env`. */
  env: NonNullable<HarnessOptions["env"]>
  /** Install in the Harness registry; wraps the mutating tools. */
  extension: Extension
  /**
   * Raises the epoch of the existing Space `name` to this driver's session
   * epoch now (not at first use), cutting off older drivers' processes at
   * once. A Space already at or above the epoch is left alone; no-op when the
   * Space does not exist or no session epoch is configured. Throws
   * `StaleEpochError` if the Space refuses the epoch as stale.
   */
  raiseEpoch(name: string): Promise<void>
  /**
   * Makes the Space of `conversationId` exist and match the log now, without
   * a tool call: a lost Space is rebuilt from the conversation's newest
   * checkpoint (checkpoint mode) or from the log's workspace image
   * (transactional mode). Call it before reporting a conversation done, since
   * a Space lost after the last call would otherwise never be touched. `reader`
   * is the Harness (or any `Session`).
   */
  ensureSpace(
    reader: Pick<import("@earendil-works/pi-durable").Session, "snapshot">,
    conversationId: ConversationId,
    context: Context,
  ): Promise<void>
  /** The Space name of a conversation in this process. */
  spaceOf(conversationId: ConversationId): string | undefined
  /** Stop every file daemon pipe. */
  close(): Promise<void>
}

function hash(text: string): string {
  return createHash("sha256").update(text).digest("hex").slice(0, 16)
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}

type EventBody = RecoveryEvent extends infer E ? (E extends unknown ? Omit<E, "mode"> : never) : never

/** The session epoch from `CUA_SESSION_EPOCH`, if set to a positive integer. */
function sessionEpochFromEnv(): number | undefined {
  const value = Number(process.env.CUA_SESSION_EPOCH)
  return Number.isSafeInteger(value) && value > 0 ? value : undefined
}

/** How often streamed output re-reads the log's fence. */
const FENCE_INTERVAL_MS = 200

/** Builds the environment factory and the extension that keep Spaces consistent with the log. */
export function durableSpaces(options: DurableSpacesOptions): DurableSpaces {
  const mode = options.mode ?? "auto"
  const limits: TransactionalLimits = {
    ...DEFAULT_TRANSACTIONAL_LIMITS,
    ...Object.fromEntries(Object.entries(options.transactional ?? {}).filter(([, v]) => v !== undefined)),
  }
  const sessionEpoch = options.epoch ?? sessionEpochFromEnv()
  const marker = options.incarnationMarker ?? "/var/tmp/cua-pi-durable/incarnation"
  let reportedStale = false
  const reportStale = (error: StaleEpochError) => {
    if (reportedStale) return
    reportedStale = true
    try {
      options.onStale?.(error)
    } catch {
      // The callback must not mask the refusal.
    }
  }
  let reportedHalt = false
  const reportHalt = (error: HaltError) => {
    if (reportedHalt) return
    reportedHalt = true
    try {
      options.onHalt?.(error)
    } catch {
      // The callback must not mask the failure.
    }
  }
  const emit = (event: EventBody, in_: "checkpoint" | "transactional" = "checkpoint") =>
    options.onRecovery?.({ ...event, mode: in_ } as RecoveryEvent)
  const namespace = options.namespace ?? "default"
  const spaceName = options.spaceName ?? ((id: ConversationId) => `pi-${hash(`${namespace}:${id}`)}`)
  const wrapped = new Set(options.tools ?? ["bash", "write", "edit"])
  const rerun = new Set(options.rerun ?? ["write", "edit"])
  // After transactional mode's undo the workspace is exactly as before the call, so
  // any tool may run again by default (effects outside the workspace are not undone).
  const rerunTx = new Set(options.rerun ?? ["write", "edit", "bash", "powershell"])
  const bindings = new Map<number, Promise<Binding>>()
  const names = new Map<number, string>()

  const makeEnv = (name: string) => (binding: Binding) =>
    new SpacesExecutionEnv({
      ...options.env,
      spacesd: () => options.spaces.spacesd(name),
      id: `cua-space:${name}`,
      cwd: options.cwd,
      epoch: () => binding.epoch,
      onStale: (error) => {
        binding.stale ??= error
        reportStale(error)
      },
    })

  /** The Space of `conversationId`, created or forked on first use. */
  const bind = (conversationId: ConversationId, state: Readonly<SpaceState> | undefined): Promise<Binding> => {
    const key = conversationId as number
    let binding = bindings.get(key)
    if (binding === undefined) {
      binding = open(conversationId, state)
      bindings.set(key, binding)
      binding.catch(() => bindings.delete(key))
    }
    return binding.then((b) => {
      if ((state?.epoch ?? 0) > b.epoch && state?.owner === key) b.epoch = state.epoch ?? 0
      return b
    })
  }

  /**
   * Where a Space records which life it is in. A Space without one is new:
   * just created, or rebuilt (or forked) by a process that did not finish.
   * It is written only after the Space holds its checkpoint's state.
   */
  const MARKER = marker

  const open = async (conversationId: ConversationId, state: Readonly<SpaceState> | undefined): Promise<Binding> => {
    const own = state?.owner === (conversationId as number) && state.space !== undefined
    const name = own ? state.space! : spaceName(conversationId)
    const last = state?.checkpoints?.at(-1)
    // A fork's record is its parent's, as of the fork entry.
    const forked = !own && last !== undefined && state?.space !== undefined && state.space !== name
    let incarnation: string | undefined
    if (await options.spaces.exists(name)) {
      const read = await sh(await options.spaces.spacesd(name), `cat ${MARKER} 2>/dev/null`)
      if (read.code === 0 && read.stdout.trim() !== "") incarnation = read.stdout.trim()
    }
    if (incarnation === undefined) {
      if (forked) {
        await options.snapshots.fork(last!.id, state!.space!, name)
        emit({ kind: "forked", conversationId: conversationId as number, space: name, from: last!.id })
      } else if (last) {
        // This conversation's Space is gone (or its rebuild was cut off):
        // build it again from the newest checkpoint, which only a provider
        // that keeps checkpoints outside the Space can do.
        try {
          await options.snapshots.fork(last.id, name, name)
        } catch (error) {
          const text = message(error)
          // The checkpoint is not anywhere: it was kept only inside the Space that was lost (no
          // CheckpointStore). Trying again cannot bring it back, so a session driver stops for good
          // with the reason; any other failure (an unreachable store) may pass and stays a crash.
          const gone = /not.?found|no such file|path not found|does not exist/i.test(text)
          const failure = gone
            ? `its checkpoints were kept only inside Space ${name}, which was lost: use a CheckpointStore or mode "transactional" (checkpoint ${last.id}: ${text})`
            : `Space ${name} was lost and checkpoint ${last.id} cannot be restored: ${text}`
          if (gone && sessionEpoch !== undefined) {
            const halt = new HaltError(failure, true, { cause: error })
            reportHalt(halt)
            throw halt
          }
          throw new Error(failure)
        }
        options.spaces.invalidate?.(name)
        emit({ kind: "rebuilt", conversationId: conversationId as number, space: name, from: last.id })
      } else {
        await options.spaces.ensure(name)
      }
      incarnation = randomUUID()
      const written = await sh(
        await options.spaces.spacesd(name),
        // The working directory exists before anything (the file daemon) starts in it.
        `mkdir -p ${quote(options.cwd)} ${MARKER.slice(0, MARKER.lastIndexOf("/"))} && echo ${incarnation} > ${MARKER}.tmp && sync && mv -f ${MARKER}.tmp ${MARKER} && sync`,
      )
      if (written.code !== 0) throw new Error(`Space ${name}: recording its incarnation failed: ${written.stderr}`)
    }
    names.set(conversationId as number, name)
    // Epoch 0 would be unfenced: a conversation's commands carry at least 1.
    let epoch = Math.max(1, state?.epoch ?? 0)
    if (sessionEpoch !== undefined) {
      // Never below the session epoch; above it only to stay ahead of a bump an earlier
      // driver of this session made (a restore), so its processes are fenced as well.
      const recorded = own ? (state?.epoch ?? 0) : 0
      epoch = recorded >= sessionEpoch ? recorded + 1 : sessionEpoch
      if (recorded < sessionEpoch) {
        // The Space may already stand at exactly this epoch without this log having raised it: an
        // earlier session's driver on a shared durable Space bumped itself past its session epoch (a
        // restore or revert), or an earlier driver of this session died before recording its bump.
        // Running at the same epoch would not fence that driver's processes, and the keys of this
        // log's commands (conversation:call:n@epoch) could meet the other log's, so the Space would
        // replay its results instead of running them. A Space that is already past it is left to
        // the fence below, which refuses a stale driver.
        try {
          const current = (await getExecRecord(await options.spaces.spacesd(name), "bind-probe")).epoch
          if (current === epoch) epoch = current + 1
        } catch {
          // Unreachable: the fence below reports it.
        }
      }
    }
    const binding = new Binding(name, epoch, incarnation, makeEnv(name))
    if (sessionEpoch !== undefined) {
      await fenceSpace(name, epoch)
    } else {
      // Fail fast, before any call: restores later raise the epoch, so the Space and SDK
      // must support it. Re-asserting the current epoch changes nothing.
      const current = (await getExecRecord(await options.spaces.spacesd(name), "capability-probe")).epoch
      await fenceSpace(name, Math.max(1, current))
    }
    return binding
  }

  /** Raises the Space's epoch to `epoch`, cutting off older drivers; a Space that is ahead means this driver is stale. */
  const fenceSpace = async (name: string, epoch: number) => {
    try {
      await advanceEpoch(await options.spaces.spacesd(name), epoch)
    } catch (error) {
      if (staleEpochOf(error) || haltOf(error)) throw error
      if (isStaleEpoch(error)) {
        const stale = staleFromMessage(epoch, String((error as Error)?.message ?? error))
        reportStale(stale)
        throw stale
      }
      // Anything else leaves the fence unestablished: halt, never record it as a tool result.
      const text = String((error as Error)?.message ?? error)
      const unsupported = /not a JSON-callable|unknown method|unimplemented|not implemented|not supported|no such method/i.test(text)
      const halt = new HaltError(
        unsupported
          ? `this cua SDK or cua-spacesd does not support ProcessService/AdvanceEpoch (${text}); the epoch fence needs a newer @trycua/cua (the SDK built from the same release as cua-spacesd with the process.exec_ledger capability)`
          : `raising the Space epoch to ${epoch} failed: ${text}`,
        unsupported,
        { cause: error },
      )
      reportHalt(halt)
      throw halt
    }
  }

  /**
   * Whether the Space's epoch is ahead of `binding`'s (a newer driver advanced it), and if the
   * Space cannot be reached at all, why. A failed call is classified here, not by its text.
   */
  const probeSpace = async (name: string, epoch: number): Promise<{ ahead?: StaleEpochError; unreachable?: string }> => {
    if (sessionEpoch === undefined) return {}
    try {
      const current = (await getExecRecord(await options.spaces.spacesd(name), "stale-probe")).epoch
      if (current > epoch) return { ahead: staleFromMessage(epoch, `epoch ${epoch} is older than the Space's current epoch ${current}`) }
    } catch (error) {
      return { unreachable: message(error) }
    }
    return {}
  }
  const spaceIsAhead = async (binding: Binding): Promise<StaleEpochError | undefined> => (await probeSpace(binding.name, binding.epoch)).ahead

  /**
   * A call failed and the Space cannot be reached: it is gone or its host is down. That is not
   * a tool result the model should see (it would only see transport errors for every later call
   * and the run would end `done` with the work undone). Halt the driver, transiently, so the
   * re-driver resumes it once the Space is back; an unrecorded call is simply run again.
   */
  const haltIfUnreachable = async (name: string, epoch: number) => {
    const { unreachable } = await probeSpace(name, epoch)
    if (unreachable === undefined) return
    const halt = new HaltError(`the Space ${name} cannot be reached (${unreachable}); the session is resumed when it is back`, false)
    reportHalt(halt)
    throw halt
  }

  const env = async (target: EnvTarget, context: Context): Promise<ExecutionEnv> => {
    const state = await target.read.snapshot(SpaceDoc, target.conversationId, context)
    const binding = await bind(target.conversationId, state)
    if (target.cwd !== undefined) binding.env.cwd = target.cwd
    return binding.env
  }

  /** Records a checkpoint taken after `api`'s call and fills the previous record's result entry. */
  const record = async (api: ToolExecutionApi, binding: Binding, checkpoint: CheckpointRecord, context: Context) => {
    const state = await api.snapshot(SpaceDoc, api.conversationId, context)
    const previous = state?.checkpoints?.at(-1)
    let entryId: number | undefined
    if (previous?.taskId !== undefined && previous.entryId === undefined) {
      const task = await api.getTask(previous.taskId as never, context)
      const outcome = (task?.state as { outcome?: { result?: { entryId?: number } } } | undefined)?.outcome
      entryId = outcome?.result?.entryId
    }
    await api.commit(async (tx) => {
      const doc = await tx.doc(SpaceDoc, api.conversationId)
      doc.owner = api.conversationId as number
      doc.space = binding.name
      doc.epoch = binding.epoch
      if (doc.checkpoints === undefined) doc.checkpoints = []
      const last = doc.checkpoints.at(-1)
      if (last && entryId !== undefined && last.entryId === undefined) last.entryId = entryId
      if (!doc.checkpoints.some((c) => c.id === checkpoint.id && c.callId === checkpoint.callId)) {
        doc.checkpoints.push(checkpoint)
      }
      // Bounded: older records stay readable through the document's history.
      while (doc.checkpoints.length > 64) doc.checkpoints.shift()
    }, context)
  }

  /** Make the conversation own its Space record, redo a pending restore, and take the base checkpoint. */
  const prepare = async (api: ToolExecutionApi, binding: Binding, context: Context): Promise<SpaceState> => {
    let state = (await api.snapshot(SpaceDoc, api.conversationId, context)) ?? {}
    const conversation = api.conversationId as number
    if (state.owner !== conversation || state.space !== binding.name) {
      // First use, or a fork still carrying its parent's record: keep only the
      // checkpoint this Space was created from (the fork copied it).
      const inherited = state.owner !== undefined && state.owner !== conversation ? state.checkpoints?.at(-1) : undefined
      await api.commit(async (tx) => {
        const doc = await tx.doc(SpaceDoc, api.conversationId)
        doc.owner = conversation
        doc.space = binding.name
        doc.epoch = binding.epoch
        doc.checkpoints = inherited ? [{ id: inherited.id }] : (doc.checkpoints ?? []).slice()
        delete doc.restoring
      }, context)
      state = (await api.snapshot(SpaceDoc, api.conversationId, context)) ?? {}
    }
    if (state.restoring) {
      // Finishing a restore an earlier driver began. The intent's epoch can be below the Space's
      // floor: this driver's own epoch (bind raised the Space past the recorded one) is above it,
      // and fencing the Space at the older epoch again would refuse this driver as stale.
      await restore(api, binding, state.restoring.to, Math.max(state.restoring.epoch, binding.epoch), context)
      state = (await api.snapshot(SpaceDoc, api.conversationId, context)) ?? {}
    }
    if ((state.checkpoints ?? []).length === 0) {
      const id = await options.snapshots.checkpoint(binding.name, checkpointLabel(binding.name, "base"))
      await record(api, binding, { id }, context)
      state = (await api.snapshot(SpaceDoc, api.conversationId, context)) ?? {}
    }
    return state
  }

  /** Restore `checkpoint` under a new `epoch`: intent first, then fence, restore, fence again, then clear. */
  const restore = async (
    api: ToolExecutionApi,
    binding: Binding,
    checkpoint: string,
    epoch: number,
    context: Context,
    decision?: CallState["restored"],
  ) => {
    // The intent, and the decision of the call that asked for it, in one commit.
    await api.commit(async (tx) => {
      const doc = await tx.doc(SpaceDoc, api.conversationId)
      doc.epoch = epoch
      doc.restoring = { to: checkpoint, epoch }
      if (decision) (await tx.doc(CallDoc, api.taskId)).restored = decision
    }, context)
    binding.epoch = epoch
    // Kill everything fenced with an older epoch (commands, file daemon pipe)
    // before the files change under them. A Space that is already gone has
    // nothing left to fence.
    if (await options.spaces.exists(binding.name)) {
      await fenceSpace(binding.name, epoch)
    }
    await options.snapshots.restore(binding.name, checkpoint)
    // A whole-machine restore is a new machine: new connection, and the
    // checkpoint's own (older) epoch, raised again before anything runs.
    options.spaces.invalidate?.(binding.name)
    await binding.env.reconnect()
    await fenceSpace(binding.name, epoch)
    await api.commit(async (tx) => {
      const doc = await tx.doc(SpaceDoc, api.conversationId)
      delete doc.restoring
    }, context)
  }

  const interrupted = (tool: string, to: CheckpointRecord): ToolExecutionResult => {
    const point = to.entryId !== undefined ? `entry ${to.entryId}` : to.callId ? `call ${to.callId}` : "its base state"
    const text = `Tool ${tool} was interrupted and its outcome is unknown. The sandbox was restored to its state at ${point}, so nothing this call did remains; run it again if it is still needed.`
    return {
      content: [],
      isError: true,
      diagnostics: [{ severity: "error", code: "interrupted", message: text }],
    }
  }

  /**
   * A view of `api` for one tool run: commands carry the call's idempotency keys,
   * the streamed output and last details are captured (to replay an adopted
   * outcome), and both are held back once the fence says a newer driver owns the
   * session.
   */
  const scopedApi = (api: ToolExecutionApi, binding: Binding, prefix: string, keyEpoch?: number) => {
    let output = ""
    const dropped: Skip = { bytes: 0, newlines: 0, endsWithNewline: false }
    let details: JsonValue | undefined
    let halted: StaleEpochError | undefined
    let lastCheck = 0
    const scoped: ToolExecutionApi = {
      ...api,
      env: binding.env.withRequestIds(prefix, keyEpoch),
      output(chunk, skipped) {
        halted ??= options.fence?.stale ?? binding.stale
        if (halted === undefined && options.fence !== undefined && Date.now() - lastCheck >= FENCE_INTERVAL_MS) {
          lastCheck = Date.now()
          // Latches `fence.stale` for the next chunk; the commit is the authority.
          void options.fence.check().catch(() => {})
        }
        if (halted !== undefined) return
        const text = typeof chunk === "string" ? chunk : new TextDecoder().decode(chunk)
        if (skipped) {
          dropped.bytes += skipped.bytes + Buffer.byteLength(output)
          dropped.newlines += skipped.newlines + (output.split("\n").length - 1)
          dropped.endsWithNewline = skipped.endsWithNewline
          output = ""
        }
        output += text
        if (output.length > MAX_CAPTURE) {
          const cut = output.slice(0, output.length - MAX_CAPTURE)
          dropped.bytes += Buffer.byteLength(cut)
          dropped.newlines += cut.split("\n").length - 1
          dropped.endsWithNewline = cut.endsWith("\n")
          output = output.slice(-MAX_CAPTURE)
        }
        api.output(chunk, skipped)
      },
      async details(value, detailsContext) {
        await options.fence?.check()
        if (binding.stale) throw binding.stale
        details = value
        return api.details(value, detailsContext)
      },
    }
    return {
      scoped,
      halted: () => halted ?? options.fence?.stale ?? binding.stale,
      /** The outcome as recorded in the log. */
      record(outcome: NonNullable<CallState["outcome"]>): NonNullable<CallState["outcome"]> {
        const recorded = {
          ...outcome,
          output,
          incarnation: binding.incarnation,
          ...(dropped.bytes > 0 ? { dropped } : {}),
        }
        if (details !== undefined) recorded.details = details
        return recorded
      },
    }
  }

  /**
   * A refusal by a fence is never a tool result: throw it up through the tool so
   * the driver halts. Also catches a Space that moved ahead without the tool
   * noticing (a file tool whose daemon was killed by the newer driver).
   */
  const haltIfStale = async (
    binding: Binding,
    capturing: { halted(): StaleEpochError | undefined },
    thrown?: unknown,
    result?: ToolExecutionResult,
  ) => {
    const stale = staleEpochOf(thrown) ?? capturing.halted()
    if (stale) {
      reportStale(stale)
      throw stale
    }
    if (thrown !== undefined || result?.isError) {
      const probe = await probeSpace(binding.name, binding.epoch)
      if (probe.ahead) {
        reportStale(probe.ahead)
        throw probe.ahead
      }
      if (probe.unreachable !== undefined) await haltIfUnreachable(binding.name, binding.epoch)
    }
    try {
      await options.fence?.check()
    } catch (error) {
      const refused = staleEpochOf(error)
      if (refused) reportStale(refused)
      throw error
    }
  }

  /**
   * What the exec ledger says about a call's command: it finished cleanly (adopt its outcome),
   * still runs in this epoch (reattach), never started (run it), or its fate is unknown. A
   * record killed by a signal or left from an older epoch is unknown: the epoch advance that
   * cut off the old driver kills what it ran, so its effects are partial.
   */
  const ledgerVerdict = (
    ledger: Awaited<ReturnType<typeof getExecRecord>>["record"],
    epoch: number,
  ): "run" | "reattach" | "adopt" | "unknown" => {
    if (ledger === undefined) return "run"
    if (ledger.state === "running") return ledger.epoch >= epoch ? "reattach" : "unknown"
    if (ledger.state === "exited" && ledger.exitCode !== undefined && ledger.signal === undefined) return "adopt"
    return "unknown"
  }

  /** Records that the conversation uses checkpoint mode from now on. */
  const fallBack = async (
    api: ToolExecutionApi,
    binding: Binding,
    context: Context,
    reason: string,
    from: "checkpoint" | "transactional",
  ) => {
    await api.commit(async (tx) => {
      const doc = await tx.doc(SpaceDoc, api.conversationId)
      doc.mode = "checkpoint"
      doc.modeReason = reason
    }, context)
    emit({ kind: "mode", conversationId: api.conversationId as number, callId: api.callId, to: "checkpoint", reason }, from)
    void binding
  }

  /**
   * The mode of this call. Explicit modes are used as asked. `auto` picks
   * transactional unless the conversation already fell back, already has
   * checkpoints, or its Space is a VM that a whole-machine checkpoint serves
   * (or of unknown kind with only whole-machine checkpoints available). Workspace size,
   * tooling and leftover processes are judged where they are measured, in `transact`.
   */
  const chooseMode = async (
    binding: Binding,
    api: ToolExecutionApi,
    context: Context,
  ): Promise<"transactional" | "checkpoint"> => {
    const state = await api.snapshot(SpaceDoc, api.conversationId, context)
    if (state?.mode === "checkpoint") return "checkpoint"
    if (mode === "transactional") return "transactional"
    const image = await api.snapshot(ImageDoc, api.conversationId, context)
    if ((state?.checkpoints?.length ?? 0) > 0 && image?.files === undefined) return "checkpoint"
    binding.kind ??= options.spaces.kind?.(binding.name) ?? Promise.resolve(undefined)
    const kind = (await binding.kind)?.toLowerCase()
    const whole = options.snapshots.wholeMachine === true
    if (whole && (kind === undefined || kind === "microvm" || kind === "vm" || kind === "macos-vm" || kind === "macos")) {
      await fallBack(api, binding, context, kind === undefined ? "Space kind unknown and checkpoints capture the whole machine" : `a ${kind} Space with whole-machine checkpoints`, "transactional")
      return "checkpoint"
    }
    return "transactional"
  }

  const interruptedTx = (tool: string): ToolExecutionResult => {
    const text = `Tool ${tool} was interrupted and its outcome is unknown. The workspace was restored to its state before the call, so nothing this call did remains; run it again if it is still needed.`
    return {
      content: [],
      isError: true,
      diagnostics: [{ severity: "error", code: "interrupted", message: text }],
    }
  }

  /**
   * Transactional mode: the image of the workspace in the log is the truth. A
   * call commits its start (with any change made to the workspace outside the
   * calls) and then its outcome together with the diff it made; a crash leaves
   * the Space somewhere between the two images, and recovery makes it equal the
   * newest committed one. Returns undefined when the conversation fell back to
   * checkpoint mode before the call started.
   */
  const transact = async (
    tool: ToolRegistration,
    args: unknown,
    api: ToolExecutionApi,
    context: Context,
    binding: Binding,
    call: CallState,
    trace: (point: string) => void,
    label: string,
  ): Promise<ToolExecutionResult | undefined> => {
    const conversation = api.conversationId as number
    const prefix = `${conversation}:${api.callId}`
    const root = options.cwd
    const spacesd = () => options.spaces.spacesd(binding.name)
    const event = (e: EventBody) => emit(e, "transactional")
    const readImage = async (): Promise<ImageState> =>
      ((await api.snapshot(ImageDoc, api.conversationId, context)) ?? {}) as ImageState

    let image = await readImage()
    if (image.stale) {
      throw new Error(
        "transactional mode: the workspace outgrew its limits during an earlier call, so the log no longer holds it; use mode \"checkpoint\" for this conversation",
      )
    }
    const state = await api.snapshot(SpaceDoc, api.conversationId, context)
    if (state?.restoring) {
      throw new Error("transactional mode: this conversation has a checkpoint restore in progress; finish it in checkpoint mode first")
    }
    const lived = call.started?.incarnation ?? call.outcome?.incarnation
    const lost = lived !== undefined && lived !== binding.incarnation
    /** Does the image describe the workspace of this life of this Space? */
    const here = image.owner === conversation && image.space === binding.name && image.incarnation === binding.incarnation

    const replay = async (outcome: NonNullable<CallState["outcome"]>): Promise<ToolExecutionResult> => {
      if (outcome.output) api.output(outcome.output, outcome.dropped)
      if (outcome.details !== undefined) await api.details(outcome.details, context)
      if (outcome.thrown !== undefined) throw new Error(outcome.thrown)
      return outcome.result as unknown as ToolExecutionResult
    }

    /** Makes the Space equal the image. */
    const revert = async (why: "reverted" | "reimaged") => {
      const done = await reconcile(await spacesd(), root, image, limits)
      if (why === "reverted") {
        event({ kind: "reverted", conversationId: conversation, callId: api.callId, files: done.written + done.deleted, epoch: binding.epoch })
      } else {
        event({ kind: "reimaged", conversationId: conversation, callId: api.callId, files: done.written + done.deleted })
      }
    }

    // 1. Outcome already recorded: its diff is in the image. The Space has it unless it was replaced.
    if (call.outcome !== undefined) {
      if (!here && image.files !== undefined) await revert("reimaged")
      emit({ kind: "adopted", conversationId: conversation, callId: api.callId, via: "result" }, "transactional")
      return replay(call.outcome)
    }

    const bash = tool.name === "bash" || tool.name === "powershell"
    const again = rerunTx.has(tool.name) || tool.replay === "safe"
    let undo = call.started?.undo
    let keyEpoch = call.started?.keyEpoch ?? call.started?.epoch ?? binding.epoch
    let started = call.started !== undefined

    // 2. Effects may have started in an earlier attempt.
    if (call.started !== undefined && undo === undefined) {
      // The keys are those of the epoch the call started in, which may be older than this
      // driver's: the daemon replays a key whatever epoch the new request carries.
      let unknown = true
      if (!lost && bash) {
        const { record: ledger } = await getExecRecord(await spacesd(), `${prefix}:0@${keyEpoch}`)
        const verdict = ledgerVerdict(ledger, binding.epoch)
        if (verdict !== "unknown") {
          unknown = false
          if (verdict === "reattach") emit({ kind: "reattached", conversationId: conversation, callId: api.callId }, "transactional")
          if (verdict === "adopt") emit({ kind: "adopted", conversationId: conversation, callId: api.callId, via: "ledger" }, "transactional")
        }
      }
      if (unknown) {
        // The decision and the new epoch are committed before anything is undone.
        const epoch = binding.epoch + 1
        const decided = { rerun: again }
        undo = decided
        await api.commit(async (tx) => {
          const doc = await tx.doc(SpaceDoc, api.conversationId)
          doc.owner = conversation
          doc.space = binding.name
          doc.epoch = epoch
          const record = await tx.doc(CallDoc, api.taskId)
          record.started = { epoch, incarnation: binding.incarnation, undo: decided }
          delete record.outcome
        }, context)
        binding.epoch = epoch
        keyEpoch = epoch
        trace("undo-committed")
      }
    }
    if (undo !== undefined) {
      // Fence everything older (also redone after a crash), then undo.
      await fenceSpace(binding.name, binding.epoch)
      trace("undoing")
      await revert("reverted")
      trace("reverted")
      if (!undo.rerun) {
        const outcome = { result: interruptedTx(tool.name) as unknown as JsonValue, incarnation: binding.incarnation }
        await api.commit(async (tx) => {
          const record = await tx.doc(CallDoc, api.taskId)
          record.started = { epoch: binding.epoch, incarnation: binding.incarnation, ...(keyEpoch !== binding.epoch ? { keyEpoch } : {}) }
          record.outcome = outcome
        }, context)
        trace("outcome")
        return outcome.result as unknown as ToolExecutionResult
      }
      await api.commit(async (tx) => {
        const record = await tx.doc(CallDoc, api.taskId)
        // The re-run's commands carry the keys of `keyEpoch` (decided when the undo was committed);
        // `bind` may since have raised this driver's epoch past it. The next driver looks the call up
        // under the key epoch recorded here, so it must not be lost.
        record.started = { epoch: binding.epoch, incarnation: binding.incarnation, ...(keyEpoch !== binding.epoch ? { keyEpoch } : {}) }
      }, context)
      started = true
    }

    // 3. Mark the start. Whatever changed the workspace since the last call joins the image here,
    // so the image the call's own diff is taken against is the workspace before the call.
    if (!started) {
      const sp = await spacesd()
      let before: Awaited<ReturnType<typeof scan>> | undefined
      try {
        if (!here && image.files !== undefined) {
          // A Space the image does not describe (replaced, rebuilt, forked): the image wins.
          await revert("reimaged")
          before = image.files
        } else {
          before = await scan(sp, root, limits)
        }
      } catch (error) {
        if (error instanceof TransactionalLimitExceeded || error instanceof TransactionalUnsupported) {
          if (mode === "transactional") throw error
          await fallBack(api, binding, context, error.message, "transactional")
          return undefined
        }
        throw error
      }
      const drift = diff(image.files, before)
      const blobs = await capture(sp, root, before, drift.upsert, new Set(Object.keys(image.blobs ?? {})))
      await api.commit(async (tx) => {
        const space = await tx.doc(SpaceDoc, api.conversationId)
        space.owner = conversation
        space.space = binding.name
        space.epoch = binding.epoch
        const doc = await tx.doc(ImageDoc, api.conversationId)
        doc.owner = conversation
        doc.space = binding.name
        doc.incarnation = binding.incarnation
        applyDiff(doc, before, drift, blobs)
        const record = await tx.doc(CallDoc, api.taskId)
        record.started = { epoch: binding.epoch, incarnation: binding.incarnation }
        delete record.outcome
      }, context)
      image = await readImage()
    }
    trace("started")

    // 4. Run.
    const capturing = scopedApi(api, binding, prefix, keyEpoch !== binding.epoch ? keyEpoch : undefined)
    let result: ToolExecutionResult | undefined
    let thrown: string | undefined
    try {
      result = await tool.execute(args as never, capturing.scoped, context)
    } catch (error) {
      if (context.abortSignal?.aborted) throw error
      await haltIfStale(binding, capturing, error)
      thrown = message(error)
    }
    if (context.abortSignal?.aborted && thrown === undefined) return result
    await haltIfStale(binding, capturing, undefined, result)
    trace("executed")

    // 5. The call's diff and its outcome, in one commit.
    const sp = await spacesd()
    let after: Awaited<ReturnType<typeof scan>> | undefined
    let leaving: string | undefined
    try {
      after = await scan(sp, root, limits)
    } catch (error) {
      if (!(error instanceof TransactionalLimitExceeded || error instanceof TransactionalUnsupported)) throw error
      leaving = error.message
    }
    if (leaving === undefined && mode === "auto" && bash) {
      const background = await backgroundProcesses(sp, root)
      if (background > 0) leaving = `the call left ${background} process${background === 1 ? "" : "es"} running in the workspace`
    }
    const recorded = capturing.record(thrown !== undefined ? { thrown } : { result: result as unknown as JsonValue })
    if (after !== undefined && leaving === undefined) {
      const changes = diff(image.files, after)
      const blobs = await capture(sp, root, after, changes.upsert, new Set(Object.keys(image.blobs ?? {})))
      trace("diffed")
      await api.commit(async (tx) => {
        const doc = await tx.doc(ImageDoc, api.conversationId)
        doc.incarnation = binding.incarnation
        applyDiff(doc, after, changes, blobs)
        ;(await tx.doc(CallDoc, api.taskId)).outcome = recorded
      }, context)
      trace("outcome")
    } else if (mode === "transactional") {
      // Explicit mode has no fallback: the outcome is kept, the image is marked unusable.
      await api.commit(async (tx) => {
        ;(await tx.doc(ImageDoc, api.conversationId)).stale = true
        ;(await tx.doc(CallDoc, api.taskId)).outcome = recorded
      }, context)
      trace("outcome")
    } else {
      // Auto: the outcome and the switch to checkpoint mode commit together; the checkpoint follows.
      await api.commit(async (tx) => {
        const space = await tx.doc(SpaceDoc, api.conversationId)
        space.mode = "checkpoint"
        space.modeReason = leaving!
        ;(await tx.doc(CallDoc, api.taskId)).outcome = recorded
      }, context)
      trace("outcome")
      emit({ kind: "mode", conversationId: conversation, callId: api.callId, to: "checkpoint", reason: leaving! }, "transactional")
      const id = await options.snapshots.checkpoint(binding.name, label)
      trace("checkpointed")
      await record(api, binding, { id, callId: api.callId, taskId: api.taskId as number }, context)
      trace("recorded")
    }
    if (thrown !== undefined) throw new Error(thrown)
    return result
  }

  const execute = async (
    tool: ToolRegistration,
    args: unknown,
    api: ToolExecutionApi,
    context: Context,
  ): Promise<ToolExecutionResult> => {
    const conversation = api.conversationId as number
    // A stale driver must not even open the Space.
    try {
      await options.fence?.check()
    } catch (error) {
      const refused = staleEpochOf(error)
      if (refused) reportStale(refused)
      throw error
    }
    const binding = await bind(api.conversationId, await api.snapshot(SpaceDoc, api.conversationId, context))
    return binding.serialize(async () => {
      const trace = (point: string) => options.trace?.(point, api.callId)
      trace("call")
      // A newer driver owns the session: stop before touching anything.
      try {
        await options.fence?.check()
      } catch (error) {
        const refused = staleEpochOf(error)
        if (refused) reportStale(refused)
        throw error
      }
      if (binding.stale) throw binding.stale
      const call = (await api.snapshot(CallDoc, api.taskId, context)) ?? {}
      const label = checkpointLabel(binding.name, hash(`${conversation}:${api.callId}`))

      if (mode !== "checkpoint") {
        const used = await chooseMode(binding, api, context)
        if (used === "transactional") {
          const done = await transact(tool, args, api, context, binding, call, trace, label)
          if (done !== undefined) return done
          // Fell back to checkpoint mode before the call started: carry on below.
        }
      }

      /** Replays a recorded outcome through `api`, as the tool reported it. */
      const replay = async (outcome: NonNullable<CallState["outcome"]>): Promise<ToolExecutionResult> => {
        if (outcome.output) api.output(outcome.output, outcome.dropped)
        if (outcome.details !== undefined) await api.details(outcome.details, context)
        if (outcome.thrown !== undefined) throw new Error(outcome.thrown)
        return outcome.result as unknown as ToolExecutionResult
      }

      /**
       * Records the outcome, then checkpoints the state after it: a crash in
       * between adopts the outcome and takes the checkpoint then. `at` reuses
       * an existing checkpoint (the state was just restored to it).
       */
      const finish = async (outcome: NonNullable<CallState["outcome"]>, at?: string): Promise<ToolExecutionResult> => {
        await api.commit(async (tx) => {
          const doc = await tx.doc(CallDoc, api.taskId)
          doc.outcome = outcome
        }, context)
        const id = at ?? (await options.snapshots.checkpoint(binding.name, label))
        await record(api, binding, { id, callId: api.callId, taskId: api.taskId as number }, context)
        return replay(outcome)
      }

      // An outcome recorded in an earlier life of the Space whose checkpoint
      // was never recorded: the Space was rebuilt from an older checkpoint,
      // so the call's effects are gone.
      const recorded = (await api.snapshot(SpaceDoc, api.conversationId, context))?.checkpoints ?? []
      const lostWithSpace =
        (call.outcome?.incarnation !== undefined || call.started?.incarnation !== undefined) &&
        (call.outcome?.incarnation ?? call.started?.incarnation) !== binding.incarnation &&
        !recorded.some((c) => c.callId === api.callId)

      // 1. Outcome already recorded: adopt it.
      if (call.outcome !== undefined && !lostWithSpace) {
        emit({ kind: "adopted", conversationId: conversation, callId: api.callId, via: "result" })
        // Checkpoints are idempotent by label: this takes the one the crash
        // may have prevented, or finds it.
        const id = await options.snapshots.checkpoint(binding.name, label)
        await record(api, binding, { id, callId: api.callId, taskId: api.taskId as number }, context)
        return replay(call.outcome)
      }

      let state = await prepare(api, binding, context)
      const prefix = `${conversation}:${api.callId}`
      let keyEpoch =
        call.restored?.epoch === binding.epoch ? binding.epoch : (call.started?.keyEpoch ?? call.started?.epoch ?? binding.epoch)

      // A restore this call asked for has completed (`prepare` finishes a
      // pending one): carry out the decision made before it.
      if (call.restored !== undefined && call.restored.epoch === binding.epoch && !call.restored.rerun) {
        return finish({ result: interrupted(tool.name, call.restored.to) as unknown as JsonValue }, call.restored.to.id)
      }

      // 2. Effects may have started in an earlier attempt under this epoch
      // (or in a Space that has since been lost).
      // A different epoch with no restore for this call means the session epoch moved on
      // (a new driver took over); a restore this call asked for is already carried out.
      const carriedOut = call.restored !== undefined && call.restored.epoch === binding.epoch
      if ((call.started !== undefined && (call.started.epoch === binding.epoch || !carriedOut)) || lostWithSpace) {
        let unknown = true
        if (!lostWithSpace && (tool.name === "bash" || tool.name === "powershell")) {
          const spacesd = await options.spaces.spacesd(binding.name)
          const { record: ledger } = await getExecRecord(spacesd, `${prefix}:0@${keyEpoch}`)
          const verdict = ledgerVerdict(ledger, binding.epoch)
          if (verdict !== "unknown") {
            unknown = false
            if (verdict === "reattach") emit({ kind: "reattached", conversationId: conversation, callId: api.callId })
            if (verdict === "adopt") emit({ kind: "adopted", conversationId: conversation, callId: api.callId, via: "ledger" })
          }
        }
        if (unknown) {
          const to = { ...state.checkpoints!.at(-1)! }
          const epoch = binding.epoch + 1
          const again = rerun.has(tool.name) || tool.replay === "safe"
          await restore(api, binding, to.id, epoch, context, { to, epoch, rerun: again })
          emit({ kind: "restored", conversationId: conversation, callId: api.callId, checkpoint: to.id, epoch })
          state = (await api.snapshot(SpaceDoc, api.conversationId, context)) ?? state
          keyEpoch = binding.epoch
          if (!again) return finish({ result: interrupted(tool.name, to) as unknown as JsonValue }, to.id)
        }
      }

      // 3. Run (or rerun after a restore), marking that effects may start.
      if (call.started?.epoch !== binding.epoch || call.started.incarnation !== binding.incarnation) {
        await api.commit(async (tx) => {
          const doc = await tx.doc(CallDoc, api.taskId)
          doc.started = {
            epoch: binding.epoch,
            incarnation: binding.incarnation,
            ...(keyEpoch !== binding.epoch ? { keyEpoch } : {}),
          }
          delete doc.outcome
        }, context)
      }
      trace("started")
      const capturing = scopedApi(api, binding, prefix, keyEpoch !== binding.epoch ? keyEpoch : undefined)
      const scoped = capturing.scoped
      // The live output already reached `api`; `finish` must not replay it.
      const settle = async (outcome: NonNullable<CallState["outcome"]>) => {
        const recorded = capturing.record(outcome)
        await api.commit(async (tx) => {
          ;(await tx.doc(CallDoc, api.taskId)).outcome = recorded
        }, context)
        trace("outcome")
        const id = await options.snapshots.checkpoint(binding.name, label)
        trace("checkpointed")
        await record(api, binding, { id, callId: api.callId, taskId: api.taskId as number }, context)
        trace("recorded")
        if (outcome.thrown !== undefined) throw new Error(outcome.thrown)
        return outcome.result as unknown as ToolExecutionResult
      }
      let result: ToolExecutionResult
      try {
        result = await tool.execute(args as never, scoped, context)
      } catch (error) {
        if (context.abortSignal?.aborted) throw error
        await haltIfStale(binding, capturing, error)
        return settle({ thrown: message(error) })
      }
      if (context.abortSignal?.aborted) return result
      await haltIfStale(binding, capturing, undefined, result)
      return settle({ result: result as unknown as JsonValue })
    })
  }

  const extension = defineExtension({
    name: "cua-durable-spaces",
    wraps: [...wrapped].map((name) =>
      wrapTool({ name } as ToolRegistration, (tool) => ({
        ...tool,
        // Recovery is decided here (adopt, reattach, restore), so Pi may
        // always hand an interrupted call back to this wrapper.
        replay: "safe" as const,
        executionMode: "sequential" as const,
        execute: async (args: unknown, api: ToolExecutionApi, context: Context) => {
          try {
            return await execute(tool, args, api, context)
          } catch (error) {
            // A failure before or around the tool (binding, scanning, checkpointing) on a Space that is gone.
            if (sessionEpoch !== undefined && !staleEpochOf(error) && !haltOf(error) && !context.abortSignal?.aborted) {
              const conversation = api.conversationId as number
              await haltIfUnreachable(names.get(conversation) ?? spaceName(api.conversationId), Math.max(1, sessionEpoch))
            }
            throw error
          }
        },
      })),
    ),
  })

  return {
    env,
    extension,
    async ensureSpace(reader, conversationId, context) {
      const state = await reader.snapshot(SpaceDoc, conversationId, context)
      if (state?.space === undefined) return // never used a Space
      const binding = await bind(conversationId, state)
      const image = await reader.snapshot(ImageDoc, conversationId, context)
      if (image?.files !== undefined && state.mode !== "checkpoint" && image.incarnation !== binding.incarnation) {
        await binding.serialize(async () => {
          const done = await reconcile(await options.spaces.spacesd(binding.name), options.cwd, image as ImageState, limits)
          emit({ kind: "reimaged", conversationId: conversationId as number, callId: "", files: done.written + done.deleted }, "transactional")
        })
      }
    },
    async raiseEpoch(name) {
      if (sessionEpoch === undefined || !(await options.spaces.exists(name))) return
      const { epoch } = await getExecRecord(await options.spaces.spacesd(name), "raise-probe")
      if (epoch < sessionEpoch) await fenceSpace(name, sessionEpoch)
    },
    spaceOf(conversationId) {
      return names.get(conversationId as number)
    },
    async close() {
      for (const binding of bindings.values()) {
        try {
          await (await binding).env.close()
        } catch {
          // Never opened.
        }
      }
    },
  }
}
