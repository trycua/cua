/**
 * The headless "Teleport an app…" state machine, shared by every client
 * (the Spaces app, the OpenKoalaBots examples). No DOM, no Node, no
 * framework: a pure reducer over plain data, plus selectors.
 *
 * The data mirrors the cua SDK's `Teleport.catalog` / `plan` / `run`
 * records (camelCase). Hosts that return the Rust core's JSON (snake_case,
 * for example a Tauri command) convert it with {@link entryFromCore},
 * {@link planFromCore} and {@link runEventFromCore}.
 */

import { type InstallCuaPrompt, requiresCuaApp } from "./install.js"

/** What teleport can do with an app. */
export type Capability = "full" | "install_only" | "unsupported"

/** What a teleport moves. */
export type Move = "app_only" | "app_with_files" | "app_with_state"

/** Credential-shaped state the signed-in state move leaves out by default,
 * opted into one group at a time: `sign_ins` is the session cookies (what
 * keeps the app signed in). Each group's items are secrets in the plan. */
export type SensitiveGroup = "sign_ins" | "passwords" | "history"

/** One row of "Teleport an app…". */
export interface CatalogEntry {
  id: string
  name: string
  hostPath?: string | null
  hostAppId?: string | null
  version?: string | null
  capability: Capability
  /** Why it is unsupported, or a caveat. */
  reason?: string | null
  /** Offered moves, in UI order (empty when unsupported). */
  moves: Move[]
  providerId?: string | null
  /** The opt-in groups the signed-in state move offers, UI order. */
  sensitiveGroups?: SensitiveGroup[]
  /** `manifest` (pinned install), `image`, `space`, or none. */
  installSource?: "manifest" | "image" | "space" | null
  installId?: string | null
  installVersion?: string | null
  launchBin?: string | null
  lastUsedMs?: number | null
  /** The SDK's JSON for this entry (what `plan` reads back). */
  json: string
}

/** Options a user picks before planning. */
export interface PlanOptions {
  moves: Move
  files: string[]
  /** Provider manifest items; null/undefined = the provider's default. */
  stateItems?: string[] | null
  /** With the default items: also these opt-in groups (each a secret). */
  sensitiveGroups?: SensitiveGroup[]
  scope?: "tabs" | "full"
  launch?: boolean
}

export type ConsentKind = "install" | "file" | "folder" | "state" | "secret"

/** One line of the consent screen. */
export interface ConsentItem {
  kind: ConsentKind
  key: string
  label: string
  detail: string
  bytes: number
  sensitive: boolean
}

/** Exactly what a teleport will do and move. */
export interface Plan {
  app: CatalogEntry
  spaceId: string
  moves: Move
  steps: { kind: string; summary: string }[]
  consent: ConsentItem[]
  sensitive: boolean
  totalBytes: number
  warnings: string[]
  /** This Space is reached through a relay connection that predates
   * end-to-end sealing: a secret this plan sends would cross it in the
   * clear (S1). Needs `acknowledgeRelayPlaintext` the same way a secret
   * needs `acknowledgeSensitive`. */
  relayUnsealed: boolean
  /** The SDK's JSON for this plan (what `run` reads back). */
  json: string
}

export type RunPhase = "started" | "progress" | "finished" | "failed" | "done"

export interface RunEvent {
  step: number
  steps: number
  kind: string
  phase: RunPhase
  detail: string
  doneBytes: number
  totalBytes: number
}

export interface RunReport {
  appId: string
  installed: string[]
  sent: string[]
  imported: string[]
  skipped: string[]
  launched: boolean
}

export interface Consent {
  approved: boolean
  acknowledgeSensitive: boolean
  /** "Save to Keyvault": keep the captured session sealed in the user's
   * own Cua Keyvault after delivery, instead of forgetting it. */
  saveToKeyvault: boolean
  /** [`Plan.relayUnsealed`]'s warning acknowledged (S1). */
  acknowledgeRelayPlaintext: boolean
}

/**
 * What a picker needs from its host: the SDK (Node), a Tauri command
 * layer, or an HTTP server in front of the SDK.
 */
export interface TeleportHost {
  /** The classified catalog (the host narrows it to the Space). */
  catalog(): Promise<CatalogEntry[]>
  plan(entry: CatalogEntry, options: PlanOptions): Promise<Plan>
  run(plan: Plan, consent: Consent, onEvent: (event: RunEvent) => void): Promise<RunReport>
  /** An icon as a URL (`data:` or `blob:`), or null. */
  icon?(entry: CatalogEntry): Promise<string | null>
  /** A native file/folder chooser; absent in plain browsers. */
  chooseFiles?(): Promise<string[]>
}

export type Step = "loading" | "pick" | "options" | "planning" | "consent" | "running" | "done" | "error"

export interface PickerState {
  step: Step
  spaceName: string
  entries: CatalogEntry[] | null
  query: string
  selectedId: string | null
  entry: CatalogEntry | null
  move: Move | null
  files: string[]
  /** The opt-in groups checked on the options step. */
  sensitive: SensitiveGroup[]
  plan: Plan | null
  acknowledged: boolean
  /** "Save to Keyvault" checked (shown only for a sensitive plan). */
  saveToKeyvault: boolean
  /** [`Plan.relayUnsealed`]'s warning acknowledged (S1). */
  acknowledgedRelayPlaintext: boolean
  events: RunEvent[]
  report: RunReport | null
  error: string | null
  /** Set when the error is the Keyvault's "needs the Cua app" refusal:
   * show this prompt (install button) instead of the raw error. */
  installPrompt: InstallCuaPrompt | null
  /** Where "back" from an error goes. */
  errorBack: Step
}

export type PickerEvent =
  | { type: "loaded"; entries: CatalogEntry[] }
  | { type: "failed"; message: string; cause?: unknown }
  | { type: "query"; query: string }
  | { type: "select"; id: string }
  | { type: "choose"; id?: string }
  | { type: "preselect"; entry: CatalogEntry; files?: string[] }
  | { type: "move"; move: Move }
  | { type: "files"; files: string[] }
  | { type: "remove-file"; path: string }
  | { type: "sensitive"; group: SensitiveGroup; value: boolean }
  | { type: "plan" }
  | { type: "planned"; plan: Plan }
  | { type: "acknowledge"; value: boolean }
  | { type: "save-to-keyvault"; value: boolean }
  | { type: "acknowledge-relay-plaintext"; value: boolean }
  | { type: "confirm" }
  | { type: "progress"; event: RunEvent }
  | { type: "finished"; report: RunReport }
  | { type: "back" }

/** The initial state: loading the catalog. */
export function initialState(spaceName: string): PickerState {
  return {
    step: "loading",
    spaceName,
    entries: null,
    query: "",
    selectedId: null,
    entry: null,
    move: null,
    files: [],
    sensitive: [],
    plan: null,
    acknowledged: false,
    saveToKeyvault: false,
    acknowledgedRelayPlaintext: false,
    events: [],
    report: null,
    error: null,
    installPrompt: null,
    errorBack: "pick",
  }
}

/** The honest default: the least that moves (files when some were dropped). */
export function defaultMove(entry: CatalogEntry, files: readonly string[] = []): Move | null {
  if (files.length > 0 && entry.moves.includes("app_with_files")) return "app_with_files"
  return entry.moves[0] ?? null
}

function toOptions(s: PickerState, entry: CatalogEntry, files: string[]): PickerState {
  return {
    ...s,
    step: "options",
    entry,
    selectedId: entry.id,
    move: defaultMove(entry, files),
    files,
    sensitive: [],
    plan: null,
    acknowledged: false,
    saveToKeyvault: false,
    acknowledgedRelayPlaintext: false,
    error: null,
    installPrompt: null,
  }
}

/** The reducer. Invalid transitions return the state unchanged. */
export function reduce(s: PickerState, e: PickerEvent): PickerState {
  switch (e.type) {
    case "loaded": {
      const enabled = e.entries.find((x) => x.capability !== "unsupported")
      // A preselected flow (options, planning, consent) keeps its step.
      if (s.step !== "loading") return { ...s, entries: e.entries }
      return { ...s, step: "pick", entries: e.entries, selectedId: s.selectedId ?? enabled?.id ?? null, error: null, installPrompt: null }
    }
    case "failed": {
      const back: Step =
        s.step === "planning" ? "options" : s.step === "running" ? "consent" : s.entries ? "pick" : "loading"
      const installPrompt = requiresCuaApp(e.cause) ?? requiresCuaApp(e.message)
      return { ...s, step: "error", error: e.message, installPrompt, errorBack: back }
    }
    case "query": {
      if (s.step !== "pick") return s
      const next = { ...s, query: e.query }
      const visible = visibleEntries(next)
      const keep = visible.some((x) => x.id === s.selectedId)
      return keep ? next : { ...next, selectedId: visible.find((x) => x.capability !== "unsupported")?.id ?? null }
    }
    case "select":
      return s.step === "pick" ? { ...s, selectedId: e.id } : s
    case "choose": {
      if (s.step !== "pick") return s
      const id = e.id ?? s.selectedId
      const entry = s.entries?.find((x) => x.id === id)
      if (!entry || entry.capability === "unsupported") return s
      return toOptions(s, entry, [])
    }
    case "preselect":
      if (e.entry.capability === "unsupported") {
        return {
          ...s,
          step: "error",
          entry: e.entry,
          error: `${e.entry.name} cannot be teleported: ${e.entry.reason ?? "unsupported"}`,
          installPrompt: null,
          errorBack: "pick",
        }
      }
      return toOptions(s, e.entry, e.files ?? [])
    case "move":
      return s.step === "options" && s.entry?.moves.includes(e.move) ? { ...s, move: e.move } : s
    case "files":
      if (s.step !== "options") return s
      return { ...s, files: [...new Set([...s.files, ...e.files])] }
    case "remove-file":
      return s.step === "options" ? { ...s, files: s.files.filter((f) => f !== e.path) } : s
    case "sensitive": {
      // Only while the checkboxes show: options, signed-in state move.
      const offered = s.entry?.sensitiveGroups?.includes(e.group) ?? false
      if (s.step !== "options" || s.move !== "app_with_state" || !offered) return s
      const rest = s.sensitive.filter((g) => g !== e.group)
      return { ...s, sensitive: e.value ? [...rest, e.group] : rest }
    }
    case "plan":
      return s.step === "options" && canPlan(s) ? { ...s, step: "planning", error: null, installPrompt: null } : s
    case "planned":
      return s.step === "planning"
        ? {
            ...s,
            step: "consent",
            plan: e.plan,
            acknowledged: false,
            saveToKeyvault: false,
            acknowledgedRelayPlaintext: false,
          }
        : s
    case "acknowledge":
      return s.step === "consent" ? { ...s, acknowledged: e.value } : s
    case "save-to-keyvault":
      // Only meaningful (and only shown) for a plan with something
      // sensitive to save.
      return s.step === "consent" && s.plan?.sensitive ? { ...s, saveToKeyvault: e.value } : s
    case "acknowledge-relay-plaintext":
      return s.step === "consent" ? { ...s, acknowledgedRelayPlaintext: e.value } : s
    case "confirm":
      return s.step === "consent" && canConfirm(s) ? { ...s, step: "running", events: [] } : s
    case "progress":
      return s.step === "running" ? { ...s, events: [...s.events, e.event].slice(-200) } : s
    case "finished":
      return s.step === "running" ? { ...s, step: "done", report: e.report } : s
    case "back":
      switch (s.step) {
        case "options":
          return s.entries ? { ...s, step: "pick", plan: null } : s
        case "consent":
          return {
            ...s,
            step: "options",
            plan: null,
            acknowledged: false,
            saveToKeyvault: false,
            acknowledgedRelayPlaintext: false,
          }
        case "error":
          return { ...s, step: s.errorBack === "loading" ? "loading" : s.errorBack, error: null, installPrompt: null }
        default:
          return s
      }
  }
}

/** Case-insensitive search over name, id and host app id; every word must match. */
export function searchEntries(entries: readonly CatalogEntry[], query: string): CatalogEntry[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean)
  return entries.filter((e) => {
    const hay = `${e.name} ${e.id} ${e.hostAppId ?? ""}`.toLowerCase()
    return words.every((w) => hay.includes(w))
  })
}

export function visibleEntries(s: PickerState): CatalogEntry[] {
  return searchEntries(s.entries ?? [], s.query)
}

/** The pick list's sections: recents, then available, then unavailable. */
export function sections(s: PickerState): { title: string; entries: CatalogEntry[] }[] {
  const visible = visibleEntries(s)
  const recents = visible.filter((e) => e.lastUsedMs != null && e.capability !== "unsupported")
  const rest = visible.filter((e) => !recents.includes(e))
  return [
    { title: "Recent", entries: recents },
    { title: "Apps", entries: rest.filter((e) => e.capability !== "unsupported") },
    { title: "Not available", entries: rest.filter((e) => e.capability === "unsupported") },
  ].filter((x) => x.entries.length > 0)
}

export function canPlan(s: PickerState): boolean {
  if (!s.entry || !s.move || !s.entry.moves.includes(s.move)) return false
  return s.move !== "app_with_files" || s.files.length > 0
}

/** The groups the plan asks for: the checked ones, in the entry's order,
 * only with the signed-in state move. */
export function planSensitive(s: PickerState): SensitiveGroup[] {
  if (s.move !== "app_with_state") return []
  return (s.entry?.sensitiveGroups ?? []).filter((g) => s.sensitive.includes(g))
}

export function canConfirm(s: PickerState): boolean {
  return (
    s.plan != null &&
    (!s.plan.sensitive || s.acknowledged) &&
    (!s.plan.relayUnsealed || s.acknowledgedRelayPlaintext)
  )
}

/** Overall run progress in [0, 1]. */
export function progress(s: PickerState): number {
  const last = s.events[s.events.length - 1]
  if (!last) return 0
  if (last.phase === "done") return 1
  const within = last.totalBytes > 0 ? Math.min(1, last.doneBytes / last.totalBytes) : last.phase === "finished" ? 1 : 0
  return Math.min(1, (last.step + within) / Math.max(1, last.steps))
}

export const CAPABILITY_LABEL: Record<Capability, string> = {
  full: "App and signed-in state",
  install_only: "App, empty or with files",
  unsupported: "Not available",
}

export const MOVE_LABEL: Record<Move, string> = {
  app_only: "Just the app",
  app_with_files: "The app with files or folders",
  app_with_state: "The app with its signed-in state",
}

export function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  const units = ["KB", "MB", "GB", "TB"]
  let v = n / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v >= 10 ? Math.round(v) : v.toFixed(1)} ${units[i]}`
}

// ---- the Rust core's JSON (snake_case) ---------------------------------

type Json = Record<string, unknown>
const str = (v: unknown): string | null => (typeof v === "string" ? v : null)
const num = (v: unknown): number => (typeof v === "number" ? v : 0)

/** A `cua_teleport::ux::CatalogEntry` (serde JSON) as a {@link CatalogEntry}. */
export function entryFromCore(j: Json): CatalogEntry {
  const install = (j.install ?? null) as Json | null
  const launch = (j.launch ?? null) as Json | null
  const kind = install ? str(install.kind) : null
  return {
    id: String(j.id),
    name: String(j.name),
    hostPath: str(j.host_path),
    hostAppId: str(j.host_app_id),
    version: str(j.version),
    capability: j.capability as Capability,
    reason: str(j.reason),
    moves: (j.moves as Move[]) ?? [],
    providerId: str(j.provider_id),
    sensitiveGroups: Array.isArray(j.sensitive_groups) ? (j.sensitive_groups as SensitiveGroup[]) : [],
    installSource: kind === "manifest" || kind === "image" || kind === "space" ? kind : null,
    installId: install && kind === "manifest" ? str(install.id) : null,
    installVersion: install && kind === "manifest" ? str(install.version) : null,
    launchBin: launch ? str(launch.bin) : null,
    lastUsedMs: typeof j.last_used_ms === "number" ? j.last_used_ms : null,
    json: JSON.stringify(j),
  }
}

function stepSummary(step: Json): string {
  const kind = String(step.kind)
  const list = (v: unknown) => (Array.isArray(v) ? (v as string[]) : [])
  switch (kind) {
    case "install":
      return `Install ${list(step.ids).join(", ")} (pinned, verified)`
    case "send_files":
      return `Send ${list(step.paths).length} item(s) to ~/Downloads/${String(step.subdir)}`
    case "import_state":
      return `Import ${list(step.items).length} ${String(step.provider_id)} item(s)`
    case "launch": {
      const files = list(step.files)
      return files.length ? `Open ${String(step.bin)} with ${files.length} item(s)` : `Open ${String(step.bin)}`
    }
    default:
      return kind
  }
}

const STEP_NAME: Record<string, string> = {
  install: "install",
  send_files: "files",
  import_state: "state",
  launch: "launch",
}

/** A `cua_teleport::ux::TeleportPlan` (serde JSON) as a {@link Plan}. */
export function planFromCore(j: Json): Plan {
  const steps = (j.steps as Json[]) ?? []
  return {
    app: entryFromCore(j.app as Json),
    spaceId: String(j.space_id),
    moves: j.moves as Move,
    steps: steps.map((s) => ({ kind: STEP_NAME[String(s.kind)] ?? String(s.kind), summary: stepSummary(s) })),
    consent: ((j.consent as Json[]) ?? []).map((c) => ({
      kind: c.kind as ConsentKind,
      key: String(c.key),
      label: String(c.label),
      detail: String(c.detail),
      bytes: num(c.bytes),
      sensitive: Boolean(c.sensitive),
    })),
    sensitive: Boolean(j.sensitive),
    relayUnsealed: Boolean(j.relay_unsealed),
    totalBytes: num(j.total_bytes),
    warnings: (j.warnings as string[]) ?? [],
    json: JSON.stringify(j),
  }
}

/** A `cua_teleport::ux::RunEvent` (serde JSON) as a {@link RunEvent}. */
export function runEventFromCore(j: Json): RunEvent {
  return {
    step: num(j.step),
    steps: num(j.steps),
    kind: String(j.kind),
    phase: j.phase as RunPhase,
    detail: String(j.detail ?? ""),
    doneBytes: num(j.done_bytes),
    totalBytes: num(j.total_bytes),
  }
}

/** A `cua_teleport::ux::RunReport` (serde JSON) as a {@link RunReport}. */
export function runReportFromCore(j: Json): RunReport {
  const list = (v: unknown) => (Array.isArray(v) ? (v as string[]) : [])
  return {
    appId: String(j.app_id),
    installed: list(j.installed),
    sent: list(j.sent),
    imported: list(j.imported),
    skipped: list(j.skipped),
    launched: Boolean(j.launched),
  }
}
