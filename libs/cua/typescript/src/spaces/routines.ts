/**
 * `@trycua/cua/spaces/routines`: recurring tasks a Bot runs on a schedule.
 * No native library, no Node imports, so it runs in a browser or a webview too.
 *
 * A routine is a saved prompt plus a clock, owned by one Bot (one long-lived
 * agent thread). When it comes due the store hands it to a
 * {@link RoutineRunner}, which gives that Bot another turn in the shared
 * Space. The store keeps three things apart:
 *
 * - **persistence**: every mutation is written through a {@link RoutineStorage}
 *   (a JSON array, the same shape the Swift and Rust SDKs write);
 * - **the clock**: {@link RoutineStore.tick} takes the instant to evaluate, so
 *   the scheduler is testable without sleeping;
 * - **the firing**: delegated to the runner, so the store never sees a Space.
 *
 * ```ts
 * import { RoutineStore, memoryStorage } from "@trycua/cua/spaces/routines"
 *
 * const routines = new RoutineStore(memoryStorage(), {
 *   fire: async (r) => ({ kind: "started", runId: await startTurn(r.botID, r.prompt) }),
 * })
 * routines.create({ botID: "inbox", title: "Morning sweep", prompt: "Triage the inbox",
 *                   schedule: { kind: "dailyAt", hour: 8, minute: 0 } })
 * routines.startScheduler(15_000)
 * ```
 *
 * A scheduler that was asleep fires a due routine **once** on waking, never
 * once per missed slot, and a refused firing still uses its slot.
 */

/** The three recurrence shapes. `weekday` is 1 = Sunday … 7 = Saturday. */
export type RoutineSchedule =
  | { kind: "everyMinutes"; minutes: number }
  | { kind: "dailyAt"; hour: number; minute: number }
  | { kind: "weeklyOn"; weekday: number; hour: number; minute: number }

/** One routine. Dates are ISO 8601 (UTC, whole seconds). */
export interface Routine {
  id: string
  /** The Bot that runs it. A routine never spans Bots; a group chat does. */
  botID: string
  title: string
  /** The text handed to the Bot when it fires. */
  prompt: string
  schedule: RoutineSchedule
  isEnabled: boolean
  createdAt: string
  /** When the scheduler last started a firing (persisted, so a restart does not re-fire the past). */
  lastFiredAt?: string
  /** The agent run the last firing produced. */
  lastRunID?: string
  /** The scheduler's words about the last firing: started, refused, failed. */
  lastOutcome?: string
}

/** What happened when a routine fired. A refusal is not a failure. */
export type RoutineFiring =
  | { kind: "started"; runId: string }
  | { kind: "refused"; reason: string }
  | { kind: "failed"; reason: string }

/** Gives a Bot its routine turn. Never throws: a refusal is a result. */
export interface RoutineRunner {
  fire(routine: Routine): Promise<RoutineFiring>
}

/** Where the routine list lives. `load` returns `undefined` when nothing was saved. */
export interface RoutineStorage {
  load(): string | undefined
  save(json: string): void
}

/** One line of the scheduler's log, newest first. */
export interface FiringRecord {
  routineID: string
  title: string
  at: string
  firing: RoutineFiring
}

/** Marks a routine-originated turn in a transcript. */
export const ROUTINE_PREFIX = "[routine]"

/** The text a runner hands the Bot: `[routine] <title>: <prompt>`. */
export function routineTurnText(routine: Pick<Routine, "title" | "prompt">): string {
  return `${ROUTINE_PREFIX} ${routine.title}: ${routine.prompt}`
}

/** A {@link RoutineStorage} in memory. */
export function memoryStorage(initial?: string): RoutineStorage & { value: string | undefined } {
  const s = {
    value: initial,
    load: () => s.value,
    save: (json: string) => {
      s.value = json
    },
  }
  return s
}

/** ISO 8601 in UTC with whole seconds, the portable form every SDK reads. */
export function isoSeconds(d: Date): string {
  return new Date(Math.floor(d.getTime() / 1000) * 1000).toISOString().replace(/\.\d{3}Z$/, "Z")
}

function at(d: Date, hour: number, minute: number): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate(), hour, minute, 0, 0)
}

function addDays(d: Date, n: number): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n, d.getHours(), d.getMinutes(), 0, 0)
}

/** The first slot strictly after `reference`, in local time; `undefined` for an invalid schedule. */
export function nextFireDate(schedule: RoutineSchedule, reference: Date): Date | undefined {
  switch (schedule.kind) {
    case "everyMinutes":
      if (!(schedule.minutes > 0)) return undefined
      return new Date(reference.getTime() + schedule.minutes * 60_000)
    case "dailyAt": {
      let c = at(reference, schedule.hour, schedule.minute)
      if (c.getTime() <= reference.getTime()) c = at(addDays(c, 1), schedule.hour, schedule.minute)
      return c
    }
    case "weeklyOn": {
      const want = (((schedule.weekday - 1) % 7) + 7) % 7
      const ahead = (want - reference.getDay() + 7) % 7
      let c = at(addDays(reference, ahead), schedule.hour, schedule.minute)
      if (c.getTime() <= reference.getTime()) c = at(addDays(c, 7), schedule.hour, schedule.minute)
      return c
    }
  }
}

/** When `routine` next fires after `reference`, or `undefined` when disabled. */
export function routineNextFire(routine: Routine, reference: Date): Date | undefined {
  if (!routine.isEnabled) return undefined
  const last = routine.lastFiredAt ? new Date(routine.lastFiredAt) : undefined
  const from = last && last.getTime() > reference.getTime() ? last : reference
  return nextFireDate(routine.schedule, from)
}

/**
 * Whether the scheduler should fire `routine` at `now`. Measured from the
 * last firing, not the tick, so a sleeping scheduler fires once on waking.
 */
export function isDue(routine: Routine, now: Date): boolean {
  if (!routine.isEnabled) return false
  const from = new Date(routine.lastFiredAt ?? routine.createdAt)
  const next = nextFireDate(routine.schedule, from)
  return next !== undefined && next.getTime() <= now.getTime()
}

const WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"]

/** `8:05 AM`. */
export function clockLabel(hour: number, minute: number): string {
  const h = hour % 12 === 0 ? 12 : hour % 12
  return `${h}:${String(minute).padStart(2, "0")} ${hour < 12 ? "AM" : "PM"}`
}

/** The one-line description of a schedule: `Every day at 8:00 AM`. */
export function scheduleLabel(schedule: RoutineSchedule): string {
  switch (schedule.kind) {
    case "everyMinutes": {
      const m = schedule.minutes
      if (m === 1) return "Every minute"
      if (m % 60 === 0) return m === 60 ? "Every hour" : `Every ${m / 60} hours`
      return `Every ${m} minutes`
    }
    case "dailyAt":
      return `Every day at ${clockLabel(schedule.hour, schedule.minute)}`
    case "weeklyOn":
      return `Every ${WEEKDAYS[(((schedule.weekday - 1) % 7) + 7) % 7]} at ${clockLabel(schedule.hour, schedule.minute)}`
  }
}

/** `started run <id>`, `refused: <why>`, `failed: <why>`. */
export function firingSummary(f: RoutineFiring): string {
  return f.kind === "started" ? `started run ${f.runId}` : `${f.kind}: ${f.reason}`
}

function validSchedule(s: unknown): s is RoutineSchedule {
  if (!s || typeof s !== "object") return false
  const o = s as Record<string, unknown>
  const int = (k: string) => Number.isInteger(o[k])
  switch (o.kind) {
    case "everyMinutes":
      return int("minutes")
    case "dailyAt":
      return int("hour") && int("minute")
    case "weeklyOn":
      return int("weekday") && int("hour") && int("minute")
    default:
      return false
  }
}

/** Parses a saved routine list. Throws on anything malformed rather than guessing. */
export function parseRoutines(json: string): Routine[] {
  const v = JSON.parse(json) as unknown
  if (!Array.isArray(v)) throw new Error("routines: expected a JSON array")
  return v.map((r, i) => {
    const o = r as Record<string, unknown>
    for (const k of ["id", "botID", "title", "prompt", "createdAt"]) {
      if (typeof o[k] !== "string") throw new Error(`routines[${i}].${k} is missing`)
    }
    if (!validSchedule(o.schedule)) throw new Error(`routines[${i}].schedule is not a schedule`)
    const out: Routine = {
      id: o.id as string,
      botID: o.botID as string,
      title: o.title as string,
      prompt: o.prompt as string,
      schedule: o.schedule,
      isEnabled: o.isEnabled !== false,
      createdAt: o.createdAt as string,
    }
    if (typeof o.lastFiredAt === "string") out.lastFiredAt = o.lastFiredAt
    if (typeof o.lastRunID === "string") out.lastRunID = o.lastRunID
    if (typeof o.lastOutcome === "string") out.lastOutcome = o.lastOutcome
    return out
  })
}

function newId(): string {
  return globalThis.crypto.randomUUID().toUpperCase()
}

/** Routines: storage, editing, and the clock that fires them. */
export class RoutineStore {
  routines: Routine[] = []
  /** What the scheduler did, newest first (at most 50). */
  log: FiringRecord[] = []
  private timer: ReturnType<typeof setTimeout> | undefined
  private ticking = false
  private readonly listeners = new Set<() => void>()

  constructor(
    readonly storage: RoutineStorage,
    private runner?: RoutineRunner,
  ) {
    this.load()
  }

  attach(runner: RoutineRunner): void {
    this.runner = runner
  }

  /** Called after every change. Returns an unsubscribe. */
  subscribe(fn: () => void): () => void {
    this.listeners.add(fn)
    return () => this.listeners.delete(fn)
  }

  private changed(): void {
    for (const l of this.listeners) l()
  }

  /** Reads the list back. A corrupt file starts empty, with a logged complaint. */
  load(): void {
    const raw = this.storage.load()
    if (raw === undefined || raw.trim() === "") {
      this.routines = []
      return
    }
    try {
      this.routines = parseRoutines(raw)
    } catch (e) {
      this.routines = []
      this.note(`routines file could not be read (${e instanceof Error ? e.message : String(e)}); starting empty`)
    }
  }

  save(): boolean {
    try {
      this.storage.save(JSON.stringify(this.routines, null, 2))
      return true
    } catch (e) {
      this.note(`could not save routines: ${e instanceof Error ? e.message : String(e)}`)
      return false
    }
  }

  routinesFor(botID: string): Routine[] {
    return this.routines.filter((r) => r.botID === botID).sort((a, b) => a.createdAt.localeCompare(b.createdAt))
  }

  routine(id: string): Routine | undefined {
    return this.routines.find((r) => r.id === id)
  }

  create(input: {
    botID: string
    title: string
    prompt: string
    schedule: RoutineSchedule
    enabled?: boolean
    now?: Date
  }): Routine {
    const r: Routine = {
      id: newId(),
      botID: input.botID,
      title: input.title,
      prompt: input.prompt,
      schedule: input.schedule,
      isEnabled: input.enabled ?? true,
      createdAt: isoSeconds(input.now ?? new Date()),
    }
    this.routines.push(r)
    this.save()
    this.changed()
    return r
  }

  update(routine: Routine): void {
    const i = this.routines.findIndex((r) => r.id === routine.id)
    if (i < 0) return
    this.routines[i] = routine
    this.save()
    this.changed()
  }

  delete(id: string): void {
    this.routines = this.routines.filter((r) => r.id !== id)
    this.save()
    this.changed()
  }

  /** Enable or disable without deleting; the firing history is kept. */
  setEnabled(id: string, enabled: boolean): void {
    const r = this.routine(id)
    if (!r) return
    r.isEnabled = enabled
    this.save()
    this.changed()
  }

  due(now: Date): Routine[] {
    return this.routines.filter((r) => isDue(r, now))
  }

  /** Evaluates the clock once and fires whatever is due; returns what it fired. */
  async tick(now: Date = new Date()): Promise<FiringRecord[]> {
    const fired: FiringRecord[] = []
    for (const r of this.due(now)) fired.push(await this.fire(r, now))
    return fired
  }

  /** Fires one routine now, whatever the clock says ("Run now", and the scheduler's step). */
  async fire(routine: Routine, now: Date = new Date()): Promise<FiringRecord> {
    let firing: RoutineFiring
    if (!this.runner) firing = { kind: "failed", reason: "no runner attached: not connected to a Space" }
    else {
      try {
        firing = await this.runner.fire(routine)
      } catch (e) {
        firing = { kind: "failed", reason: e instanceof Error ? e.message : String(e) }
      }
    }
    // The slot is used even on a refusal; otherwise a busy Bot is hammered every tick.
    const r = this.routine(routine.id)
    if (r) {
      r.lastFiredAt = isoSeconds(now)
      if (firing.kind === "started") r.lastRunID = firing.runId
      r.lastOutcome = firingSummary(firing)
      this.save()
    }
    const record: FiringRecord = { routineID: routine.id, title: routine.title, at: isoSeconds(now), firing }
    this.log.unshift(record)
    if (this.log.length > 50) this.log.length = 50
    this.changed()
    return record
  }

  /** One loop for every routine. Ticks never overlap. */
  startScheduler(intervalMs = 15_000, clock: () => Date = () => new Date()): void {
    this.stopScheduler()
    const loop = async () => {
      if (!this.ticking) {
        this.ticking = true
        try {
          await this.tick(clock())
        } finally {
          this.ticking = false
        }
      }
      if (this.timer !== undefined) this.timer = setTimeout(loop, intervalMs)
    }
    this.timer = setTimeout(loop, 0)
  }

  stopScheduler(): void {
    if (this.timer !== undefined) clearTimeout(this.timer)
    this.timer = undefined
  }

  get isSchedulerRunning(): boolean {
    return this.timer !== undefined
  }

  private note(text: string): void {
    this.log.unshift({ routineID: "", title: "Routines", at: isoSeconds(new Date()), firing: { kind: "failed", reason: text } })
  }
}
