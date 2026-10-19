/**
 * The local server between the web UI and the Node agent loop.
 *
 * Binds 127.0.0.1 only and requires a random bearer on every request
 * (`Authorization: Bearer …`, or `?token=` for the event socket, which a
 * browser cannot give headers). The page never sees a spacesd token:
 * streams are handed out as media tickets (`openStream`), which authorize one
 * media session and nothing else.
 */
import { randomBytes } from "node:crypto"
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs"
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http"
import type { AddressInfo } from "node:net"
import { tmpdir } from "node:os"
import { basename, extname, join, normalize } from "node:path"
import { WebSocketServer, type WebSocket } from "ws"
import type { PresenceEvent, SpaceInfo, SpacePresenceLike, TeleportManifest } from "@trycua/cua/spaces"
import { GroupChatError, GroupChatStore, type GroupChat } from "@trycua/cua/spaces/groups"
import { PresenceRoster, type PresenceEntry, type PresenceMember } from "@trycua/cua/spaces/presence"
import { RoutineStore, routineNextFire, scheduleLabel, type RoutineSchedule, type RoutineStorage } from "@trycua/cua/spaces/routines"
import { BotStore, type AgentEndpoint, type Bot, type SpacePort, type SpacesPort } from "../core/app.js"
import { BotStoreGroupMessenger, BotStoreRoutineRunner, jsonStorage } from "../core/coworkers.js"
import { assignedColorsFrom } from "../ui/coworkers.js"
import { planCall, type CreateCall, type SpacePlan } from "../core/plan.js"
import { APP_TELEPORT_MESSAGE, teleportNeedsCuaSpaces } from "../core/teleport.js"

/** What the server needs beyond the app model: tickets for the page. */
export interface StreamPort {
  /** The desktop, or one window when `windowId` is set. */
  openStream(options: {
    maxFps: number
    maxDimension: number
    windowId?: string
  }): Promise<{ wsUrl: string; ticket: string; mediaSessionId: string; codec: string; width: number; height: number; needsHeaders: boolean }>
  closeStream(mediaSessionId: string): Promise<void>
  /** The Space's streamable windows (`space.windows()`), for per-window PiP. */
  windows?(): Promise<StreamWindow[]>
}

/** One row of the Computer panel's window list. */
export interface StreamWindow {
  windowId: string
  app: string
  title: string
  /** Size in points, when known (sizes the PiP). */
  width?: number
  height?: number
  /** The app's icon as the Space's desktop shows it (a `data:` URL); absent
   * when the Space has none. */
  icon?: string
}

export interface Backend {
  spaces: SpacesPort
  /** Stream tickets for a Space handle. */
  stream: (space: SpacePort) => StreamPort
  /** The New Space wizard's Create: one `spaces.create` call, on this
   * machine or in Cua Cloud (see src/core/plan.ts). */
  create?: (call: CreateCall) => Promise<SpaceInfo>
  /** Whether Cua Cloud credentials are configured (the wizard says so). */
  cloud?: boolean
}

export interface ServerOptions {
  port?: number
  token?: string
  webRoot?: string
  pollMs?: number
  /** Where routines.json and bots.json live; in memory when unset. */
  dataDir?: string
  /** How often the routine scheduler ticks (default 15 s). */
  routineTickMs?: number
  /** A custom model endpoint for every Bot (agent_start baseUrl/model). */
  endpoint?: AgentEndpoint
}


export interface RunningServer {
  url: string
  token: string
  server: Server
  close(): Promise<void>
}

class HttpError extends Error {
  constructor(
    readonly status: number,
    message: string,
  ) {
    super(message)
  }
}

/** The SDK's "teleport ships with Cua Spaces" refusal as a 501 with its
 * message; any other error passes through. */
function needsCuaSpaces(e: unknown): never {
  const reason = teleportNeedsCuaSpaces(e)
  throw reason ? new HttpError(501, reason) : e
}

const MIME: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript",
  ".css": "text/css",
  ".wasm": "application/wasm",
  ".svg": "image/svg+xml",
  ".json": "application/json",
}

const MAX_BODY = 256 * 1024 * 1024

async function body(req: IncomingMessage, limit = MAX_BODY): Promise<Buffer> {
  const chunks: Buffer[] = []
  let size = 0
  for await (const c of req) {
    size += (c as Buffer).length
    if (size > limit) throw new HttpError(413, `body over ${limit} bytes`)
    chunks.push(c as Buffer)
  }
  return Buffer.concat(chunks)
}

async function json<T>(req: IncomingMessage): Promise<T> {
  const b = await body(req, 1 << 20)
  try {
    return (b.length ? JSON.parse(b.toString("utf8")) : {}) as T
  } catch {
    throw new HttpError(400, "invalid JSON")
  }
}

const bigintSafe = (_: string, v: unknown) => (typeof v === "bigint" ? Number(v) : v)

function send(res: ServerResponse, status: number, value: unknown): void {
  res.writeHead(status, { "content-type": "application/json", "cache-control": "no-store" })
  res.end(JSON.stringify(value, bigintSafe))
}

/** The app state behind the server. */
export class Session {
  selected: { info: SpaceInfo; space: SpacePort; store: BotStore } | null = null
  presence: { session: SpacePresenceLike; events: PresenceEvent[]; roster: PresenceRoster } | null = null
  /** Routines (persisted) and group chats, over the selected Space's Bots. */
  readonly routines: RoutineStore
  readonly groups = new GroupChatStore()
  /** Every Bot the page has named, so a routine or group can reach it after a restart. */
  private readonly bots: RoutineStorage
  readonly knownBots = new Map<string, Bot>()
  private pendingManifests = new Map<string, TeleportManifest>()
  private settledPolls = new Map<string, number>()
  readonly listeners = new Set<(event: unknown) => void>()

  constructor(
    readonly backend: Backend,
    readonly options: { dataDir?: string; endpoint?: AgentEndpoint } = {},
  ) {
    const dir = options.dataDir
    this.routines = new RoutineStore(jsonStorage(dir ? join(dir, "routines.json") : undefined))
    this.bots = jsonStorage(dir ? join(dir, "bots.json") : undefined)
    try {
      for (const b of JSON.parse(this.bots.load() ?? "[]") as Bot[]) this.knownBots.set(b.id, b)
    } catch {
      // A corrupt list starts empty; routines then report the missing Bot.
    }
  }

  /** Remembers a Bot (and saves the list). */
  know(bot: Bot): void {
    this.knownBots.set(bot.id, bot)
    this.selected?.store.register(bot)
    this.bots.save(JSON.stringify([...this.knownBots.values()], null, 2))
  }

  emit(event: unknown): void {
    for (const l of this.listeners) l(event)
  }

  requireSpace(): { info: SpaceInfo; space: SpacePort; store: BotStore } {
    if (!this.selected) throw new HttpError(409, "no Space selected")
    return this.selected
  }

  async select(id: string): Promise<SpaceInfo> {
    const all = await this.backend.spaces.list()
    const info = all.find((s) => s.id === id)
    if (!info) throw new HttpError(404, `no Space ${id}`)
    const space = await this.backend.spaces.space(id)
    const store = new BotStore(space, this.options.endpoint)
    for (const b of this.knownBots.values()) store.register(b)
    this.selected = { info, space, store }
    // One runner and messenger per Space: they start and message this Space's Bots.
    this.routines.attach(new BotStoreRoutineRunner(store))
    this.groups.attach(new BotStoreGroupMessenger(store))
    return info
  }

  rememberManifest(m: TeleportManifest): string {
    const id = randomBytes(8).toString("hex")
    this.pendingManifests.set(id, m)
    return id
  }

  takeManifest(id: string): TeleportManifest | undefined {
    const m = this.pendingManifests.get(id)
    this.pendingManifests.delete(id)
    return m
  }

  /** One tick of the single poll loop: one roster call plus one status probe
   * per unsettled thread. */
  async tick(): Promise<void> {
    const sel = this.selected
    if (!sel) return
    for (const t of sel.store.threads.values()) {
      // A settled turn is polled a few more times: a run that finishes
      // before its first poll can report its state before its last events.
      const settled = ["idle", "finished", "failed", "crashed", "stopped", "awaiting_input"].includes(t.state)
      const key = `${t.runId}:${t.revision}`
      const extra = this.settledPolls.get(key) ?? 0
      if (!settled || extra < 3) {
        if (settled) this.settledPolls.set(key, extra + 1)
        await t.refresh()
      }
    }
    await sel.store.refreshRoster()
    for (const g of this.groups.chats) await this.groups.collectReplies(g.id)
    this.emit({ type: "state", state: this.snapshot() })
  }

  snapshot(): unknown {
    const sel = this.selected
    return {
      space: sel?.info ?? null,
      roster: sel?.store.roster ?? [],
      bots: sel
        ? [...sel.store.threads.values()].map((t) => ({
            runId: t.runId,
            bot: t.bot,
            state: t.state,
            reason: t.reason,
            acceptsMessage: t.acceptsMessage,
            items: t.items,
            preview: t.preview,
          }))
        : [],
      presence: this.presence
        ? { events: this.presence.events.slice(-50), me: this.presence.roster.meId ?? null, cursors: this.presence.roster.others, colors: assignedColorsFrom(this.presence.roster.entries) }
        : null,
      knownBots: [...this.knownBots.values()],
      routines: this.routines.routines.map((r) => ({ ...r, label: scheduleLabel(r.schedule), nextFireAt: routineNextFire(r, new Date())?.toISOString() ?? null })),
      routineLog: this.routines.log.slice(0, 20),
      groups: this.groups.chats.map((g) => groupView(this.groups, g)),
    }
  }
}

function groupView(store: GroupChatStore, g: GroupChat): unknown {
  return {
    id: g.id,
    title: g.title,
    memberIDs: g.memberIDs,
    membershipLabel: g.membershipLabel,
    messages: g.messages,
    working: store.workingBots(g.id),
  }
}

function groupError<T>(f: () => T): T {
  try {
    return f()
  } catch (e) {
    if (e instanceof GroupChatError) throw new HttpError(400, e.message)
    throw e
  }
}

function isSchedule(s: unknown): s is RoutineSchedule {
  const o = s as Record<string, unknown> | null
  const int = (v: unknown, lo: number, hi: number) => Number.isInteger(v) && (v as number) >= lo && (v as number) <= hi
  if (!o || typeof o !== "object") return false
  if (o.kind === "everyMinutes") return int(o.minutes, 1, 10_080)
  if (o.kind === "dailyAt") return int(o.hour, 0, 23) && int(o.minute, 0, 59)
  if (o.kind === "weeklyOn") return int(o.weekday, 1, 7) && int(o.hour, 0, 23) && int(o.minute, 0, 59)
  return false
}

type Handler = (s: Session, req: IncomingMessage, url: URL, m: RegExpMatchArray) => Promise<unknown>

const routes: Array<[string, RegExp, Handler]> = [
  ["GET", /^\/api\/state$/, async (s) => s.snapshot()],
  ["GET", /^\/api\/spaces$/, async (s) => ({ spaces: await s.backend.spaces.list(), selected: s.selected?.info.id ?? null })],
  [
    "POST",
    /^\/api\/spaces\/add$/,
    async (s, req) => {
      // #region docs:ts-add
      const b = await json<{ url?: string; token?: string; name?: string }>(req)
      if (!b.url) throw new HttpError(400, "url is required")
      const info = await s.backend.spaces.add(b.url, b.token || undefined, b.name || undefined)
      await s.select(info.id)
      return info
      // #endregion docs:ts-add
    },
  ],
  [
    "POST",
    /^\/api\/spaces\/create$/,
    async (s, req) => {
      const b = await json<{ plan?: SpacePlan }>(req)
      if (!b.plan) throw new HttpError(400, "plan is required")
      if (!s.backend.create) throw new HttpError(501, "this server cannot create Spaces")
      let call: CreateCall
      try {
        call = planCall(b.plan)
      } catch (e) {
        throw new HttpError(400, e instanceof Error ? e.message : String(e))
      }
      if (call.on === "cloud" && !s.backend.cloud)
        throw new HttpError(501, "Cua Cloud is not signed in (run `cua auth login`, or set CUA_CLIENT_ID/SECRET)")
      const info = await s.backend.create(call)
      await s.select(info.id)
      return info
    },
  ],
  ["GET", /^\/api\/config$/, async (s) => ({ cloud: s.backend.cloud === true, create: !!s.backend.create })],
  ["POST", /^\/api\/spaces\/select$/, async (s, req) => s.select(String((await json<{ id?: string }>(req)).id ?? ""))],
  [
    "POST",
    /^\/api\/spaces\/delete$/,
    async (s, req) => {
      const id = String((await json<{ id?: string }>(req)).id ?? "")
      // #region docs:ts-delete
      const said = await s.backend.spaces.delete_(id)
      // #endregion docs:ts-delete
      if (s.selected?.info.id === id) s.selected = null
      return { deleted: said }
    },
  ],
  [
    "POST",
    /^\/api\/bots$/,
    async (s, req) => {
      const b = await json<{ name?: string; prompt?: string; agent?: string }>(req)
      if (!b.prompt) throw new HttpError(400, "prompt is required")
      const { store } = s.requireSpace()
      const name = b.name || "Koala"
      const bot = { id: `${name.toLowerCase()}-${randomBytes(3).toString("hex")}`, name, agent: b.agent || "claude-code" }
      s.know(bot)
      const t = await store.hire(bot, b.prompt)
      return { runId: t.runId, botId: bot.id }
    },
  ],
  [
    "POST",
    /^\/api\/bots\/([^/]+)\/message$/,
    async (s, req, _u, m) => {
      const t = s.requireSpace().store.thread(decodeURIComponent(m[1]))
      if (!t) throw new HttpError(404, "no such Bot")
      return t.send(String((await json<{ text?: string }>(req)).text ?? ""))
    },
  ],
  [
    "POST",
    /^\/api\/bots\/([^/]+)\/stop$/,
    async (s, _r, _u, m) => {
      const t = s.requireSpace().store.thread(decodeURIComponent(m[1]))
      if (!t) throw new HttpError(404, "no such Bot")
      return t.stop()
    },
  ],
  ["GET", /^\/api\/routines$/, async (s) => ({ routines: s.routines.routines, log: s.routines.log.slice(0, 20) })],
  [
    "POST",
    /^\/api\/routines$/,
    async (s, req) => {
      const b = await json<{ botID?: string; title?: string; prompt?: string; schedule?: unknown; enabled?: boolean }>(req)
      if (!b.botID || !s.knownBots.has(b.botID)) throw new HttpError(400, "botID must name a Bot")
      if (!b.title?.trim() || !b.prompt?.trim()) throw new HttpError(400, "title and prompt are required")
      if (!isSchedule(b.schedule)) throw new HttpError(400, "schedule is not a schedule")
      return s.routines.create({ botID: b.botID, title: b.title.trim(), prompt: b.prompt.trim(), schedule: b.schedule, enabled: b.enabled !== false })
    },
  ],
  [
    "POST",
    /^\/api\/routines\/([^/]+)$/,
    async (s, req, _u, m) => {
      const r = s.routines.routine(decodeURIComponent(m[1]))
      if (!r) throw new HttpError(404, "no such routine")
      const b = await json<{ title?: string; prompt?: string; schedule?: unknown; enabled?: boolean }>(req)
      if (b.schedule !== undefined && !isSchedule(b.schedule)) throw new HttpError(400, "schedule is not a schedule")
      const next = { ...r }
      if (typeof b.title === "string" && b.title.trim()) next.title = b.title.trim()
      if (typeof b.prompt === "string" && b.prompt.trim()) next.prompt = b.prompt.trim()
      if (b.schedule !== undefined) next.schedule = b.schedule as RoutineSchedule
      if (typeof b.enabled === "boolean") next.isEnabled = b.enabled
      s.routines.update(next)
      return s.routines.routine(r.id)
    },
  ],
  [
    "POST",
    /^\/api\/routines\/([^/]+)\/run$/,
    async (s, _r, _u, m) => {
      const r = s.routines.routine(decodeURIComponent(m[1]))
      if (!r) throw new HttpError(404, "no such routine")
      return s.routines.fire(r)
    },
  ],
  [
    "POST",
    /^\/api\/routines\/([^/]+)\/delete$/,
    async (s, _r, _u, m) => {
      s.routines.delete(decodeURIComponent(m[1]))
      return { deleted: true }
    },
  ],
  [
    "POST",
    /^\/api\/groups$/,
    async (s, req) => {
      const b = await json<{ title?: string; members?: string[] }>(req)
      const members = (b.members ?? []).filter((id) => s.knownBots.has(id))
      const title = b.title?.trim() || members.map((id) => s.knownBots.get(id)?.name ?? id).join(", ")
      return groupView(s.groups, groupError(() => s.groups.create(title, members)))
    },
  ],
  [
    "POST",
    /^\/api\/groups\/([^/]+)\/send$/,
    async (s, req, _u, m) => {
      s.requireSpace()
      const id = decodeURIComponent(m[1])
      if (!s.groups.chat(id)) throw new HttpError(404, "no such group chat")
      const text = String((await json<{ text?: string }>(req)).text ?? "").trim()
      if (!text) throw new HttpError(400, "text is required")
      return { deliveries: await s.groups.send(text, id) }
    },
  ],
  [
    "POST",
    /^\/api\/groups\/([^/]+)\/(add|remove)$/,
    async (s, req, _u, m) => {
      const id = decodeURIComponent(m[1])
      if (!s.groups.chat(id)) throw new HttpError(404, "no such group chat")
      const botID = String((await json<{ botID?: string }>(req)).botID ?? "")
      groupError(() => (m[2] === "add" ? s.groups.add(botID, id) : s.groups.remove(botID, id)))
      return groupView(s.groups, s.groups.chat(id) as GroupChat)
    },
  ],
  [
    "POST",
    /^\/api\/groups\/([^/]+)\/delete$/,
    async (s, _r, _u, m) => {
      s.groups.delete(decodeURIComponent(m[1]))
      return { deleted: true }
    },
  ],
  [
    "POST",
    /^\/api\/files$/,
    async (s, req, url) => {
      const { store } = s.requireSpace()
      const name = basename(url.searchParams.get("name") || "attachment.bin")
      const dir = mkdtempSync(join(tmpdir(), "openkoalabots-drop-"))
      try {
        const path = join(dir, name)
        writeFileSync(path, await body(req))
        return await store.attach(path, "openkoalabots")
      } finally {
        rmSync(dir, { recursive: true, force: true })
      }
    },
  ],
  [
    "POST",
    /^\/api\/teleport\/manifest$/,
    async (s, req) => {
      const b = await json<{ app?: string; scope?: string }>(req)
      const { space } = s.requireSpace()
      const manifest = await space.teleportManifest(b.app || "firefox", b.scope).catch(needsCuaSpaces)
      return { manifestId: s.rememberManifest(manifest), manifest }
    },
  ],
  [
    "POST",
    /^\/api\/teleport$/,
    async (s, req) => {
      // The human saw `manifestId`'s manifest in the page and chose items.
      // The approver refuses unless the manifest the SDK is about to act on
      // is the one they saw, item for item.
      const b = await json<{ manifestId?: string; include?: string[]; acknowledgeSensitive?: boolean }>(req)
      const seen = b.manifestId ? s.takeManifest(b.manifestId) : undefined
      if (!seen) throw new HttpError(400, "approve a manifest first (POST /api/teleport/manifest)")
      const { store } = s.requireSpace()
      const key = (m: TeleportManifest) => m.items.map((i) => i.relativePath).join("\n")
      const { receipt } = await store.teleport(
        seen.app,
        (m) => (key(m) === key(seen) ? { include: b.include, acknowledgeSensitive: b.acknowledgeSensitive === true } : undefined),
        seen.scope,
      ).catch(needsCuaSpaces)
      return receipt
    },
  ],
  [
    // "Teleport an app…" (catalog, icons, plan, run, window drags) ships
    // with Cua Spaces; these routes only say so.
    "GET",
    /^\/api\/teleport\/apps$/,
    async () => {
      throw new HttpError(501, APP_TELEPORT_MESSAGE)
    },
  ],
  [
    "POST",
    /^\/api\/teleport\/(icon|plan|run|window-drags)$/,
    async () => {
      throw new HttpError(501, APP_TELEPORT_MESSAGE)
    },
  ],
  [
    "POST",
    /^\/api\/stream$/,
    async (s, req) => {
      const b = await json<{ maxFps?: number; maxDimension?: number; windowId?: string }>(req)
      const { space } = s.requireSpace()
      const windowId = typeof b.windowId === "string" && b.windowId ? b.windowId : undefined
      if (windowId && !space.supports("window_stream")) throw new HttpError(409, "this Space does not stream single windows (window_stream)")
      if (!windowId && !space.supports("desktop_stream")) throw new HttpError(409, "this Space does not stream its desktop (desktop_stream)")
      return s.backend.stream(space).openStream({ maxFps: b.maxFps ?? 30, maxDimension: b.maxDimension ?? 1280, ...(windowId ? { windowId } : {}) })
    },
  ],
  [
    "GET",
    /^\/api\/windows$/,
    async (s) => {
      const { space } = s.requireSpace()
      const port = s.backend.stream(space)
      if (!space.supports("window_stream") || !port.windows) return { windows: [] }
      return { windows: await port.windows() }
    },
  ],
  [
    "POST",
    /^\/api\/stream\/close$/,
    async (s, req) => {
      const { space } = s.requireSpace()
      await s.backend.stream(space).closeStream(String((await json<{ mediaSessionId?: string }>(req)).mediaSessionId ?? ""))
      return { closed: true }
    },
  ],
  [
    "POST",
    /^\/api\/presence\/join$/,
    async (s, req) => {
      const b = await json<{ displayName?: string }>(req)
      const { space } = s.requireSpace()
      if (!space.supports("presence")) throw new HttpError(409, "this Space has no presence service")
      if (s.presence) await s.presence.session.leave().catch(() => {})
      // #region docs:ts-presence
      const session = await space.joinPresence(
        { id: `openkoalabots-${randomBytes(4).toString("hex")}`, displayName: b.displayName || "Operator", color: "", agent: false },
        10_000n,
      )
      // #endregion docs:ts-presence
      const me = await session.me()
      const members = await session.roster()
      s.presence = { session, events: [], roster: PresenceRoster.from(me, members) }
      void pumpPresence(s, session)
      return { me, roster: members, entries: s.presence.roster.entries }
    },
  ],
  [
    "GET",
    /^\/api\/presence$/,
    async (s) =>
      s.presence
        ? {
            me: s.presence.roster.meId ?? null,
            entries: s.presence.roster.entries,
            cursors: s.presence.roster.others,
            colors: assignedColorsFrom(s.presence.roster.entries),
            members: membersFrom(s.presence.roster.entries),
          }
        : { me: null, entries: [], cursors: [], colors: {}, members: [] },
  ],
  [
    "POST",
    /^\/api\/presence\/cursor$/,
    async (s, req) => {
      const b = await json<{ x?: number; y?: number }>(req)
      if (!s.presence) throw new HttpError(409, "join presence first")
      // #region docs:ts-cursor
      await s.presence.session.updateCursor({ displayId: "", windowId: undefined, x: b.x ?? 0, y: b.y ?? 0, visible: true, pressed: false, shape: "arrow", shapeSource: "unspecified", atMs: 0, receivedMs: 0 })
      // #endregion docs:ts-cursor
      return { ok: true }
    },
  ],
  [
    "POST",
    /^\/api\/presence\/leave$/,
    async (s) => {
      await s.presence?.session.leave()
      s.presence = null
      return { ok: true }
    },
  ],
]

/** Roster entries as the SDK's join members, to seed the page's `PresenceView`. */
function membersFrom(entries: readonly PresenceEntry[]): PresenceMember[] {
  return entries.map((e) => ({
    participant: { participantId: e.participantId, principalId: e.principalId, displayName: e.displayName, color: e.color, kind: e.agent ? "agent" : "human" },
    ...(e.cursor
      ? {
          cursor: {
            displayId: "",
            ...(e.cursor.windowId ? { windowId: e.cursor.windowId } : {}),
            x: e.cursor.x,
            y: e.cursor.y,
            visible: e.cursor.visible,
            pressed: false,
            shape: e.cursor.shape ?? "arrow",
            shapeSource: e.cursor.shape ? "hit_test" : "unspecified",
            atMs: 0,
            receivedMs: 0,
          },
        }
      : {}),
  }))
}

/** Forwards presence events to the page until the session ends. */
async function pumpPresence(s: Session, session: SpacePresenceLike): Promise<void> {
  // Bounded per wait; ends when the session is replaced or left.
  while (s.presence?.session === session) {
    let e: PresenceEvent | undefined
    try {
      e = await session.nextEvent(5_000n)
    } catch {
      return
    }
    if (!e) {
      // A quiet wait; yield so a runtime that answers at once cannot spin.
      await new Promise((r) => setTimeout(r, 100))
      continue
    }
    if (s.presence?.session !== session) return
    s.presence.events.push(e)
    if (s.presence.events.length > 500) s.presence.events.splice(0, 250)
    s.presence.roster.apply(e)
    s.emit({ type: "presence", event: e, cursors: s.presence.roster.others, colors: assignedColorsFrom(s.presence.roster.entries) })
  }
}

function serveStatic(root: string, url: URL, res: ServerResponse): boolean {
  const rel = normalize(decodeURIComponent(url.pathname)).replace(/^(\.\.[/\\])+/, "")
  let path = join(root, rel)
  if (!path.startsWith(root)) return false
  if (!existsSync(path) || statSync(path).isDirectory()) path = join(root, "index.html")
  if (!existsSync(path)) return false
  res.writeHead(200, { "content-type": MIME[extname(path)] ?? "application/octet-stream" })
  res.end(readFileSync(path))
  return true
}

export async function startServer(backend: Backend, opts: ServerOptions = {}): Promise<RunningServer> {
  const token = opts.token ?? randomBytes(24).toString("base64url")
  const session = new Session(backend, { dataDir: opts.dataDir, endpoint: opts.endpoint })
  const server = createServer(async (req, res) => {
    const url = new URL(req.url ?? "/", "http://127.0.0.1")
    if (!url.pathname.startsWith("/api/")) {
      if (opts.webRoot && serveStatic(opts.webRoot, url, res)) return
      res.writeHead(404).end()
      return
    }
    if (req.headers.authorization !== `Bearer ${token}`) return send(res, 401, { error: "missing or wrong bearer" })
    const route = routes.find(([method, re]) => method === req.method && re.test(url.pathname))
    if (!route) return send(res, 404, { error: `no route ${req.method} ${url.pathname}` })
    try {
      send(res, 200, await route[2](session, req, url, url.pathname.match(route[1]) as RegExpMatchArray))
    } catch (e) {
      const status = e instanceof HttpError ? e.status : 500
      send(res, status, { error: e instanceof Error ? e.message : String(e) })
    }
  })
  const wss = new WebSocketServer({ noServer: true })
  server.on("upgrade", (req, socket, head) => {
    const url = new URL(req.url ?? "/", "http://127.0.0.1")
    if (url.pathname !== "/events" || url.searchParams.get("token") !== token) {
      socket.write("HTTP/1.1 401 Unauthorized\r\n\r\n")
      socket.destroy()
      return
    }
    wss.handleUpgrade(req, socket, head, (ws: WebSocket) => {
      const listener = (event: unknown) => ws.send(JSON.stringify(event, bigintSafe))
      session.listeners.add(listener)
      ws.on("close", () => session.listeners.delete(listener))
      listener({ type: "state", state: session.snapshot() })
    })
  })
  await new Promise<void>((r) => server.listen(opts.port ?? 0, "127.0.0.1", r))
  const pollMs = opts.pollMs ?? 1000
  let ticking = false
  const timer = setInterval(() => {
    if (ticking) return
    ticking = true
    session
      .tick()
      .catch(() => {})
      .finally(() => (ticking = false))
  }, pollMs)
  session.routines.startScheduler(opts.routineTickMs ?? 15_000)
  const port = (server.address() as AddressInfo).port
  return {
    url: `http://127.0.0.1:${port}`,
    token,
    server,
    async close() {
      clearInterval(timer)
      session.routines.stopScheduler()
      await session.presence?.session.leave().catch(() => {})
      session.presence = null
      for (const c of wss.clients) c.terminate()
      wss.close()
      await new Promise<void>((r) => server.close(() => r()))
    },
  }
}
