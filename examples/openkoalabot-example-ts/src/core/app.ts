/**
 * OpenKoalaBots's app model on `@trycua/cua/spaces`, the TypeScript twin of the
 * Swift sample's `BotStore` / `SDKSpacesClient`:
 *
 * - attach to **one** Space and run **one long-lived agent thread per Bot**
 *   in it (never a Space per Bot);
 * - a live roster from one `agentList` per tick, joined back to local Bot
 *   identity (runs this app did not start still appear, as unmarked);
 * - each Bot's transcript folded from the run's structured events
 *   (`agentEvents`) by the SDK's `AgentTranscript`, the same rule the Swift
 *   and Tauri samples use: the agent's messages, and its install, tool,
 *   turn-end and other activity as muted groups, each attributed to the
 *   turn that caused it;
 * - a message to a Bot that is mid-turn is *refused* and the refusal is shown,
 *   never silently queued; a failed status probe degrades to `unknown`,
 *   never to health;
 * - attachments go in with `sendFile` (SHA-256 verified), a teleport needs an
 *   explicit approval callback, presence joins as a named participant.
 *
 * Besides types, only the SDK's `AgentTranscript` fold comes from the native
 * library here, so the model runs against a fake Space in unit tests.
 */
import { AgentTranscript } from "@trycua/cua"
import type {
  AgentActionReport,
  AgentRunStatus,
  AgentStartReport,
  PresenceIdentity,
  SpaceInfo,
  SpacePresenceLike,
  SpaceSendFileOptions,
  SpaceSendFileReport,
  TeleportDecision,
  TeleportManifest,
  TeleportReceipt,
} from "@trycua/cua/spaces"

/** The subset of `SpaceLike` the app uses. */
export interface SpacePort {
  id(): string
  info(): SpaceInfo
  supports(feature: string): boolean
  agentStart(agent: string, prompt: string, show: boolean | undefined, options: AgentStartOptions | undefined): Promise<AgentStartReport>
  agentStatus(runId: string, tail: number | undefined): Promise<AgentRunStatus>
  /** The `agent_events` result after `cursor`, as JSON. */
  agentEvents(runId: string, cursor: bigint | undefined, max: number | undefined): Promise<string>
  agentMessage(runId: string, text: string, force: boolean | undefined): Promise<AgentActionReport>
  agentStop(runId: string): Promise<AgentActionReport>
  agentList(): Promise<AgentRunStatus[]>
  sendFile(localPath: string, options: SpaceSendFileOptions): Promise<SpaceSendFileReport>
  teleportManifest(app: string, scope: string | undefined): Promise<TeleportManifest>
  teleport(
    app: string,
    scope: string | undefined,
    approver: { approve(manifest: TeleportManifest): TeleportDecision | undefined },
    /** The id a daemon's two-call teleport resumes with (none here). */
    requestId: string | undefined,
  ): Promise<TeleportReceipt>
  joinPresence(identity: PresenceIdentity, timeoutMs: bigint | undefined): Promise<SpacePresenceLike>
}

/** The SDK's agent options (`SpaceAgentOptions`), every field present. */
export interface AgentStartOptions {
  envFromHost: string[]
  /** More environment for the agent (none here). */
  env: Map<string, string>
  repo: string | undefined
  branch: string | undefined
  cwd: string | undefined
  model: string | undefined
  baseUrl: string | undefined
  exitWhenIdle: boolean | undefined
}

/** A custom model endpoint for every Bot this store starts. */
export interface AgentEndpoint {
  baseUrl?: string
  model?: string
  /** Provider key variables forwarded from this process's environment. */
  envFromHost?: string[]
}

export function agentOptions(e: AgentEndpoint | undefined): AgentStartOptions | undefined {
  if (!e || (!e.baseUrl && !e.model && !e.envFromHost?.length)) return undefined
  return { envFromHost: e.envFromHost ?? [], env: new Map(), repo: undefined, branch: undefined, cwd: undefined, model: e.model, baseUrl: e.baseUrl, exitWhenIdle: undefined }
}

/** The subset of `SpacesLike` the app uses. */
export interface SpacesPort {
  list(): Promise<SpaceInfo[]>
  add(url: string, token: string | undefined, name: string | undefined): Promise<SpaceInfo>
  /** Deletes a created Space's sandbox (a Space added by address is only forgotten). */
  delete_(space: string): Promise<string>
  remove(space: string): Promise<void>
  space(space: string): Promise<SpacePort>
}

export type BotState =
  | "starting"
  | "running"
  | "idle"
  | "awaiting_input"
  | "finished"
  | "failed"
  | "crashed"
  | "stopped"
  | "unknown"

/** States after which a turn has ended. */
export const SETTLED: ReadonlySet<string> = new Set(["idle", "awaiting_input", "finished", "failed", "crashed", "stopped"])

export interface Bot {
  id: string
  name: string
  agent: string
}

/**
 * One item of a thread as the page shows it. `user`: what the user typed
 * (kept locally, never the agent's echo of it); `message`: the agent's words;
 * `activity`: a muted, collapsible group of one-line steps (install, tools,
 * thinking, turn ends, notices, refusals) whose `text` is its summary.
 */
export interface ThreadItem {
  turn: number
  kind: "user" | "message" | "activity"
  text: string
  steps: string[]
}

/** The SDK fold (`AgentTranscript`), as the thread uses it. */
export interface TranscriptFold {
  absorbJson(json: string): bigint | undefined
  note(turn: number, text: string): void
  items(): Array<{ kind: string; turn: number; text: string; steps: string[] }>
  preview(): string | undefined
  revision(): bigint
}

export interface Turn {
  index: number
  message: string
  delivered: boolean
  reason: string
}

export interface RosterEntry {
  runId: string
  bot: Bot | null
  state: BotState
  reason: string
  acceptsMessage: boolean
}

export interface Delivery {
  accepted: boolean
  reason: string
}

function toState(status: string): BotState {
  const known: BotState[] = ["running", "idle", "awaiting_input", "finished", "failed", "crashed", "stopped"]
  return (known as string[]).includes(status) ? (status as BotState) : "unknown"
}

/** Largest event page read per request, and pages read per refresh. */
const EVENT_PAGE = 500
const EVENT_PAGES = 20

/** One Bot's long-lived agent thread in the shared Space. */
export class BotThread {
  readonly turns: Turn[] = []
  state: BotState = "starting"
  reason = ""
  acceptsMessage = false
  /** The run's events, folded by the SDK. Runner turn N is `turns[N - 1]`. */
  private readonly fold: TranscriptFold = new AgentTranscript()
  private cursor = 0n

  private constructor(
    readonly bot: Bot,
    readonly runId: string,
    private readonly space: SpacePort,
  ) {}

  static async hire(space: SpacePort, bot: Bot, prompt: string, options?: AgentStartOptions): Promise<BotThread> {
    // #region docs:ts-agent-start
    const report = await space.agentStart(bot.agent, prompt, false, options)
    const t = new BotThread(bot, report.runId, space)
    // #endregion docs:ts-agent-start
    t.turns.push({ index: 0, message: prompt, delivered: true, reason: "started" })
    // Start notes are setup, like the install progress that follows (turn 0).
    for (const note of report.notes) t.fold.note(0, note)
    return t
  }

  get currentTurn(): number {
    return this.turns.length - 1
  }

  /** Changes whenever the thread's items do. */
  get revision(): string {
    return `${this.turns.length}:${this.fold.revision()}`
  }

  /**
   * The thread: the user's own turns, and the fold's message and activity
   * items (its `user` items are the agent's copy of the prompt, skipped so
   * the prompt is never shown twice), each on the local turn that caused it.
   */
  get items(): ThreadItem[] {
    const out: ThreadItem[] = []
    let next = 0
    const user = (t: Turn) => out.push({ turn: t.index, kind: "user", text: t.message, steps: [] })
    for (const i of this.fold.items()) {
      if (i.kind === "user") continue
      // Install progress (turn 0) follows the first prompt.
      while (next < this.turns.length && this.turns[next].index + 1 <= Math.max(i.turn, 1)) user(this.turns[next++])
      // The SDK's grouping and summaries as they are: setup (turn 0) stays
      // its own group.
      out.push({ turn: Math.max(i.turn - 1, 0), kind: i.kind === "message" ? "message" : "activity", text: i.text, steps: [...i.steps] })
    }
    while (next < this.turns.length) user(this.turns[next++])
    return out
  }

  /** The agent's last message on one line (the roster preview), or "". */
  get preview(): string {
    return this.fold.preview() ?? ""
  }

  /** One refresh: the status, then the events written up to it (so a
   * settled status never leaves its turn's last events unread). A failed
   * probe degrades to `unknown`. */
  async refresh(): Promise<BotState> {
    let s: AgentRunStatus
    try {
      s = await this.space.agentStatus(this.runId, 0)
      for (let i = 0; i < EVENT_PAGES; i++) {
        const json = await this.space.agentEvents(this.runId, this.cursor, EVENT_PAGE)
        const page = JSON.parse(json) as { events?: unknown[]; caught_up?: boolean }
        const next = this.fold.absorbJson(json)
        if (next !== undefined) this.cursor = next
        if (page.caught_up !== false || !page.events?.length) break
      }
    } catch (e) {
      this.state = "unknown"
      this.acceptsMessage = false
      this.reason = `status probe failed: ${e instanceof Error ? e.message : String(e)}`
      return this.state
    }
    this.state = toState(s.status)
    this.reason = s.reason
    this.acceptsMessage = s.acceptsMessage
    return this.state
  }

  /** Polls until the current turn settles, bounded by `maxPolls`. */
  async settle(pollMs: number, maxPolls: number, until?: (t: BotThread) => boolean): Promise<BotState> {
    // A settled turn whose output does not (yet) satisfy `until` gets a few
    // more polls for a lagging tail, then returns rather than spinning.
    let settledPolls = 0
    for (let i = 0; i < maxPolls; i++) {
      await this.refresh()
      if (SETTLED.has(this.state)) {
        // Right after a follow-up turn exits, the runtime can briefly report
        // `crashed` ("recorded no exit status") before the exit code lands;
        // a crash is only believed once it persists.
        const provisional = this.state === "crashed" && settledPolls < 5
        if ((!provisional && (!until || until(this))) || ++settledPolls > 5) return this.state
      }
      await sleep(pollMs)
    }
    throw new Error(`turn ${this.currentTurn} of ${this.bot.name} did not settle after ${maxPolls} polls (state ${this.state})`)
  }

  /** Sends a follow-up. Refused (not queued) unless the Bot is idle. */
  async send(message: string): Promise<Delivery> {
    if (!this.acceptsMessage) {
      const reason = `${this.bot.name} is ${this.state}; a message is refused rather than queued`
      this.fold.note(this.turns.length, `Refused: ${reason}`)
      return { accepted: false, reason }
    }
    // #region docs:ts-agent-message
    const r = await this.space.agentMessage(this.runId, message, false)
    if (!r.ok) {
      this.fold.note(this.turns.length, `Refused: ${r.reason}`)
      return { accepted: false, reason: r.reason }
    }
    // #endregion docs:ts-agent-message
    this.turns.push({ index: this.turns.length, message, delivered: true, reason: r.reason })
    this.state = "running"
    this.acceptsMessage = false
    return { accepted: true, reason: r.reason }
  }

  /** The agent's messages in one turn (activity excluded). */
  outputOf(turn: number): string[] {
    return this.fold
      .items()
      .filter((i) => i.kind === "message" && i.turn === turn + 1)
      .map((i) => i.text)
  }

  async stop(): Promise<AgentActionReport> {
    const r = await this.space.agentStop(this.runId)
    if (r.ok) this.state = "stopped"
    return r
  }
}

/** The app: one Space, many Bots. */
export class BotStore {
  readonly threads = new Map<string, BotThread>()
  /** Every Bot this app knows, hired or not (routines and groups name Bots by id). */
  readonly bots = new Map<string, Bot>()
  roster: RosterEntry[] = []
  /** `agentList` round trips, so "one call per tick" is checkable. */
  rosterCalls = 0

  constructor(
    readonly space: SpacePort,
    readonly endpoint?: AgentEndpoint,
  ) {}

  /** Knows a Bot without starting it. */
  register(bot: Bot): Bot {
    this.bots.set(bot.id, bot)
    return bot
  }

  bot(id: string): Bot | undefined {
    return this.bots.get(id)
  }

  async hire(bot: Bot, prompt: string): Promise<BotThread> {
    this.register(bot)
    const t = await BotThread.hire(this.space, bot, prompt, agentOptions(this.endpoint))
    this.threads.set(t.runId, t)
    return t
  }

  thread(runId: string): BotThread | undefined {
    return this.threads.get(runId)
  }

  /** A Bot's current thread: the one it was hired into last. */
  threadFor(botId: string): BotThread | undefined {
    let last: BotThread | undefined
    for (const t of this.threads.values()) if (t.bot.id === botId) last = t
    return last
  }

  /** One roster tick: one `agentList` whatever the roster size. */
  async refreshRoster(): Promise<RosterEntry[]> {
    this.rosterCalls += 1
    let runs: AgentRunStatus[]
    try {
      runs = await this.space.agentList()
    } catch (e) {
      this.roster = this.roster.map((r) => ({ ...r, state: "unknown", acceptsMessage: false, reason: String(e) }))
      return this.roster
    }
    this.roster = runs.map((r) => ({
      runId: r.runId,
      bot: this.threads.get(r.runId)?.bot ?? null,
      state: toState(r.status),
      reason: r.reason,
      acceptsMessage: r.acceptsMessage,
    }))
    for (const r of runs) {
      const t = this.threads.get(r.runId)
      if (t) {
        t.state = toState(r.status)
        t.acceptsMessage = r.acceptsMessage
        t.reason = r.reason
      }
    }
    return this.roster
  }

  /** Attachments in: SHA-256 verified by the driver, per file. */
  async attach(localPath: string, subdir: string): Promise<SpaceSendFileReport> {
    // #region docs:ts-send-file
    const report = await this.space.sendFile(localPath, {
      targetDirectory: subdir,
      respectIgnoreFiles: true,
      conflict: "rename",
    })
    if (!report.verified) throw new Error(`send_file of ${localPath} was not verified`)
    // #endregion docs:ts-send-file
    return report
  }

  /**
   * Teleports an app session. `decide` sees exactly what would leave this
   * machine and returns the human's decision, or `undefined` to cancel.
   * Works through the Cua Spaces daemon; an embedded runtime refuses with
   * `HostCapabilityMissing` (teleport ships with Cua Spaces).
   */
  async teleport(
    app: string,
    decide: (manifest: TeleportManifest) => TeleportDecision | undefined,
    scope?: string,
  ): Promise<{ receipt: TeleportReceipt; shown: TeleportManifest[] }> {
    const shown: TeleportManifest[] = []
    // #region docs:ts-teleport
    const receipt = await this.space.teleport(app, scope, {
      approve: (m) => {
        shown.push(m)
        return decide(m)
      },
    }, undefined)
    // #endregion docs:ts-teleport
    return { receipt, shown }
  }
}

export function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms))
}
