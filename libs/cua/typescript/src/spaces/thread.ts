/**
 * Agent threads: an agent CLI running a task in a Space, with an explicit
 * isolation choice, a closed status ladder, and derived events.
 *
 * The run itself is the SDK's (`Space.agentStart` / `agentStatus` /
 * `agentMessage` / `agentStop`, the same implementation as the Spaces MCP
 * `agent_*` tools): a detached, tagged cua-spacesd process whose status is
 * read from process liveness and its recorded exit code. This module adds
 * what an app needs on top: a placement you have to choose, per-Space turn
 * serialization for shared Spaces, and `events()`.
 */

// Types only: this module loads without the native library (unit tests,
// and it never needs more than the handles it is given).
import type { SpaceCreateOptions, SpaceLike, SpacesLike } from "../native/index.js"
import { SpacesError, wrapErrors } from "./errors.js"
import { type AgentStatus, type StateEvent, type ThreadEvent, type ThreadStatus, TranscriptAdapter } from "./events.js"

/** Agent CLIs the Spaces runtime can start (the contract's `AGENT_IDS`). */
export type AgentId =
  | "claude-code"
  | "gemini-cli"
  | "google-antigravity"
  | "goose"
  | "hermes"
  | "openai-codex"
  | "openclaw"
  | "opencode"
  | "pi"

export const AGENT_IDS: readonly AgentId[] = [
  "claude-code",
  "gemini-cli",
  "google-antigravity",
  "goose",
  "hermes",
  "openai-codex",
  "openclaw",
  "opencode",
  "pi",
]

/** Statuses after which `events()` stops polling. */
export const SETTLED: ReadonlySet<AgentStatus> = new Set<AgentStatus>([
  "idle",
  "awaiting_input",
  "finished",
  "failed",
  "crashed",
])

/**
 * Where a thread's agent runs. There is no default; choosing is the point.
 */
export type ThreadPlacement =
  | {
      type: "dedicated"
      /** Image of the Space this thread creates (default: the canonical Linux image). */
      image?: string
      /** Where: `local` or `cloud` (default: the user's default location). */
      on?: string
      /** `auto` (default), `container` or `vm`. */
      kind?: string
      /** `auto` (default) or an engine the location offers for the kind. */
      runtime?: string
      /** Delete the Space when the thread closes. Default true. */
      deleteOnClose?: boolean
    }
  | {
      type: "shared"
      /** The Space (id or handle) to place this thread on. */
      space: string | SpaceLike
      /**
       * Required, and required to be literally `true`: this thread shares a
       * filesystem, a browser profile, cookies and every app login with every
       * other thread on the Space; nothing inside a Space is a security
       * boundary.
       */
      acknowledgeNoIsolation: true
    }

/** Throws unless the caller genuinely chose a placement. */
export function validatePlacement(placement: ThreadPlacement | undefined): ThreadPlacement {
  if (!placement || typeof placement !== "object") {
    throw SpacesError.usage(
      "a thread needs an explicit placement: { type: 'dedicated' } for its own Space, or " +
        "{ type: 'shared', space, acknowledgeNoIsolation: true } to share one. There is no " +
        "default because the two have different security properties.",
    )
  }
  if (placement.type === "dedicated") return placement
  if (placement.type === "shared") {
    if ((placement as { acknowledgeNoIsolation?: unknown }).acknowledgeNoIsolation !== true) {
      throw new SpacesError(
        "isolation",
        "shared placement requires acknowledgeNoIsolation: true. Threads on one Space share " +
          "a filesystem, a browser profile, cookies and every app login; a screen or a window " +
          "inside a Space is not a security boundary. This is the shared-coworker shape and it is a " +
          "fine choice — but the SDK will not make it for you.",
      )
    }
    if (!placement.space) throw SpacesError.usage("shared placement requires a space")
    return placement
  }
  throw SpacesError.usage(`unknown placement type ${JSON.stringify((placement as { type?: unknown }).type)}`)
}

/** One delivered (or refused) message. */
export interface Turn {
  id: string
  text: string
  /** `delivered` means the process started, not that the work is done. */
  state: "delivered" | "refused"
  /** The turn this one waited behind on a shared Space, if any. */
  queuedBehind?: string
  reason: string
}

/**
 * Serializes turns per Space, shared by every thread on that Space, so two
 * bots on one Space take the Space in turn instead of fighting over it.
 */
export class SpaceTurnLock {
  private tail: Promise<void> = Promise.resolve()
  private activeTurn: string | null = null

  /** The turn currently holding the Space, or null. */
  get active(): string | null {
    return this.activeTurn
  }

  /** Queues `turnId` now (call order, not scheduling order). */
  enqueue(turnId: string): { waitingBehind: string | null; acquired: Promise<() => void> } {
    const waitingBehind = this.activeTurn
    let open!: () => void
    const gate = new Promise<void>((resolve) => {
      open = resolve
    })
    const previous = this.tail
    // A failed turn must not wedge the Space forever.
    this.tail = previous.then(
      () => gate,
      () => gate,
    )
    const acquired = previous.then(
      () => this.take(turnId, open),
      () => this.take(turnId, open),
    )
    return { waitingBehind, acquired }
  }

  private take(turnId: string, open: () => void): () => void {
    this.activeTurn = turnId
    let released = false
    return () => {
      if (released) return
      released = true
      if (this.activeTurn === turnId) this.activeTurn = null
      open()
    }
  }
}

const locksById = new Map<string, SpaceTurnLock>()
/** One lock per Space id, so two handles to the same Space share it. */
export function lockFor(spaceId: string): SpaceTurnLock {
  let lock = locksById.get(spaceId)
  if (!lock) {
    lock = new SpaceTurnLock()
    locksById.set(spaceId, lock)
  }
  return lock
}

export interface StartThreadOptions {
  agent: AgentId
  /** The first task. There is no empty thread. */
  prompt: string
  placement: ThreadPlacement
  /** Your own label, echoed on the thread. */
  label?: string
  /** Open a terminal on the Space desktop tailing the run (default false). */
  show?: boolean
}

/** Starts an agent thread. `placement` is required and has no default. */
export async function startThread(spaces: SpacesLike, options: StartThreadOptions): Promise<Thread> {
  const placement = validatePlacement(options.placement)
  if (!options.prompt) throw SpacesError.usage("startThread requires a prompt")
  if (!AGENT_IDS.includes(options.agent)) {
    throw SpacesError.usage(`unknown agent ${JSON.stringify(options.agent)}; known: ${AGENT_IDS.join(", ")}`)
  }
  return wrapErrors(async () => {
    let space: SpaceLike
    let deleteOnClose = false
    if (placement.type === "dedicated") {
      const options = {
        image: placement.image,
        on: placement.on,
        kind: placement.kind,
        runtime: placement.runtime,
        name: undefined,
        wait: true,
        reuse: false,
      } as unknown as SpaceCreateOptions
      const created = await spaces.create(options)
      if (!created.space) throw SpacesError.protocol("the Space did not become ready")
      space = await spaces.space(created.space.id)
      deleteOnClose = placement.deleteOnClose !== false
    } else {
      space = typeof placement.space === "string" ? await spaces.space(placement.space) : placement.space
    }
    const started = await space.agentStart(options.agent, options.prompt, options.show ?? false, undefined)
    return new Thread({
      runId: started.runId,
      agent: options.agent,
      space,
      spaces,
      isolation: placement.type === "dedicated" ? "space" : "none",
      deleteOnClose,
      label: options.label,
      notes: started.notes,
    })
  })
}

/** Adopts a run that already exists (started by another process or the MCP). */
export async function adoptThread(spaces: SpacesLike, spaceId: string, runId: string): Promise<Thread> {
  return wrapErrors(async () => {
    const space = await spaces.space(spaceId)
    const status = await space.agentStatus(runId, 0)
    if (!status.agent) {
      // No run record: say so, and say why when the Space was unreachable.
      const unreachable = /could not reach/.test(status.reason)
      throw new SpacesError(unreachable ? "transport" : "not_found", `${runId}: ${status.reason}`)
    }
    return new Thread({
      runId,
      agent: status.agent as AgentId,
      space,
      spaces,
      isolation: "space",
      deleteOnClose: false,
    })
  })
}

export interface EventOptions {
  /** Hard bound on status polls (default 600). */
  maxPolls?: number
  /** Delay between polls (default 1000 ms). */
  intervalMs?: number
  /** Lines of output per poll (default 200). */
  tail?: number
  signal?: AbortSignal
}

/** An agent run in a Space. */
export class Thread {
  readonly runId: string
  readonly agent: AgentId
  readonly space: SpaceLike
  /** `space` when the thread has its Space to itself, `none` when shared. */
  readonly isolation: "space" | "none"
  readonly label: string | undefined
  /** Preparation steps the runtime could not complete, named. */
  readonly notes: string[]
  private readonly spaces: SpacesLike
  private readonly deleteOnClose: boolean
  private readonly adapter = new TranscriptAdapter()
  private turns = 0

  constructor(init: {
    runId: string
    agent: AgentId
    space: SpaceLike
    spaces: SpacesLike
    isolation: "space" | "none"
    deleteOnClose: boolean
    label?: string | undefined
    notes?: string[]
  }) {
    this.runId = init.runId
    this.agent = init.agent
    this.space = init.space
    this.spaces = init.spaces
    this.isolation = init.isolation
    this.deleteOnClose = init.deleteOnClose
    this.label = init.label
    this.notes = init.notes ?? []
  }

  get id(): string {
    return `thread-${this.runId}`
  }

  /** The run's status on the closed ladder, with the raw output tail. */
  async status(tail = 200): Promise<ThreadStatus> {
    const s = await wrapErrors(() => this.space.agentStatus(this.runId, tail))
    return {
      runId: s.runId || this.runId,
      status: asStatus(s.status),
      reason: s.reason,
      acceptsMessage: s.acceptsMessage,
      transcript: s.outputTail ?? "",
      desktopWindow: null,
    }
  }

  /**
   * Sends a follow-up. Turns on one Space are serialized. A run whose
   * published `acceptsMessage` is false (a turn is running, or it could not
   * be read) refuses unless `force`, and the refusal is returned, not hidden.
   */
  async send(text: string, options: { force?: boolean } = {}): Promise<Turn> {
    if (!text) throw SpacesError.usage("send requires text")
    const turnId = `${this.runId}-turn-${++this.turns}`
    const lock = lockFor(this.space.id())
    const { waitingBehind, acquired } = lock.enqueue(turnId)
    const release = await acquired
    try {
      if (!options.force) {
        // The server's rule, never re-derived from the status word here.
        const s = await wrapErrors(() => this.space.agentStatus(this.runId, 0))
        if (!s.acceptsMessage) {
          const turn: Turn = { id: turnId, text, state: "refused", reason: `${s.status}: ${s.reason}` }
          if (waitingBehind) turn.queuedBehind = waitingBehind
          return turn
        }
      }
      const r = await wrapErrors(() => this.space.agentMessage(this.runId, text, options.force ?? false))
      const turn: Turn = { id: turnId, text, state: r.ok ? "delivered" : "refused", reason: r.reason }
      if (waitingBehind) turn.queuedBehind = waitingBehind
      return turn
    } finally {
      release()
    }
  }

  /**
   * Polls the run and yields derived events: text, files, links, approval
   * prompts, and a `state` event on every status change. Stops once the run
   * settles or after `maxPolls` (bounded).
   */
  async *events(options: EventOptions = {}): AsyncGenerator<ThreadEvent> {
    const maxPolls = options.maxPolls ?? 600
    const interval = options.intervalMs ?? 1000
    let last: AgentStatus | undefined
    let seq = 0
    for (let poll = 0; poll < maxPolls; poll++) {
      if (options.signal?.aborted) return
      const s = await this.status(options.tail ?? 200)
      for (const event of this.adapter.ingest(s.transcript)) yield event
      if (s.status !== last) {
        last = s.status
        const state: StateEvent = {
          kind: "state",
          state: s.status,
          reason: s.reason,
          seq: 1_000_000 + seq++,
          observedAt: new Date().toISOString(),
          derived: false,
        }
        yield state
      }
      if (SETTLED.has(s.status)) return
      await new Promise((r) => setTimeout(r, interval))
    }
  }

  /** Stops the run; `stopped` says whether its death was witnessed. */
  async stop(): Promise<{ stopped: boolean; reason: string }> {
    const r = await wrapErrors(() => this.space.agentStop(this.runId))
    return { stopped: r.ok, reason: r.reason }
  }

  /** Stops the run and deletes a dedicated Space created for it. */
  async close(): Promise<void> {
    await this.stop().catch(() => undefined)
    if (this.deleteOnClose) await wrapErrors(() => this.spaces.delete_(this.space.id()))
  }
}

function asStatus(value: string): AgentStatus {
  const known: AgentStatus[] = ["running", "awaiting_input", "idle", "finished", "failed", "crashed", "unknown"]
  return (known as string[]).includes(value) ? (value as AgentStatus) : "unknown"
}
