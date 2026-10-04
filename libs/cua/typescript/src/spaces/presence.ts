/**
 * `@trycua/cua/spaces/presence`: the shared presence cursor, as data. No
 * native library, no Node imports.
 *
 * A {@link PresenceRoster} folds a presence session's events (`joined`,
 * `left`, `cursor_moved`) into who is here and where their cursor is, so a
 * shell can draw the other participants' cursors over the stream. Drawing is
 * the app's; this is the model. {@link waitForPresence} reads a session until
 * an event matches, bounded by time and by event count.
 *
 * ```ts
 * import { PresenceRoster, waitForPresence } from "@trycua/cua/spaces/presence"
 *
 * const session = await space.joinPresence({ id: "me", displayName: "Operator", agent: false }, 10_000n)
 * const roster = PresenceRoster.from(await session.me(), await session.roster())
 * for (;;) { const e = await session.nextEvent(1_000n); if (e) roster.apply(e); draw(roster.others) }
 * ```
 */
import type { PresenceEvent, PresenceEventSource, PresenceMember, PresenceParticipant } from "./presenceTypes.js"
export type { PresenceCursor, PresenceEvent, PresenceEventSource, PresenceMember, PresenceParticipant } from "./presenceTypes.js"
import { isCursorShape, type CursorShapeName } from "./cursorArt.js"

export { CURSOR_ART, CURSOR_SHAPES, cursorArt, cursorArtSvg, isCursorShape } from "./cursorArt.js"
export type { CursorArtShape, CursorShapeName } from "./cursorArt.js"

/** The cursor palette, in cua-spacesd's assignment order. */
export const PRESENCE_PALETTE: readonly string[] = Object.freeze([
  "#e6194b", "#3cb44b", "#4363d8", "#f58231", "#911eb4", "#46f0f0",
  "#f032e6", "#bcf60c", "#008080", "#9a6324", "#800000", "#000075",
])

/**
 * The stable presence color of an agent or other principal: a palette color
 * picked by a hash (FNV-1a, 32-bit, over the UTF-8 id), the same answer as the
 * Rust core's `presence_color` in every binding. Request it when the agent
 * joins presence (`PresenceIdentity.color`) and use it as the background of
 * its avatar, so avatar and cursor come from one source.
 */
export function presenceColor(id: string): string {
  let h = 0x811c9dc5
  for (const b of new TextEncoder().encode(id)) {
    h ^= b
    h = Math.imul(h, 0x01000193) >>> 0
  }
  return PRESENCE_PALETTE[h % PRESENCE_PALETTE.length] as string
}

/** Black or white text, whichever has the better WCAG contrast on `background` (`#rrggbb`). */
export function presenceTextColor(background: string): string {
  const hex = background.replace(/^#/, "")
  if (!/^[0-9a-fA-F]{6}$/.test(hex)) return "#000000"
  const v = parseInt(hex, 16)
  const lin = (c: number) => {
    const x = c / 255
    return x <= 0.03928 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4
  }
  const l = 0.2126 * lin((v >> 16) & 0xff) + 0.7152 * lin((v >> 8) & 0xff) + 0.0722 * lin(v & 0xff)
  return 1.05 / (l + 0.05) >= (l + 0.05) / 0.05 ? "#ffffff" : "#000000"
}

/** An agent's presence identity, requesting its stable {@link presenceColor}. */
export function agentIdentity(id: string, displayName: string): { id: string; displayName: string; color: string; agent: true } {
  return { id, displayName, color: presenceColor(id), agent: true }
}

/** One participant and their last cursor (normalized 0..1 over the streamed surface). */
export interface PresenceEntry {
  participantId: string
  /** The identity it joined with (`PresenceIdentity.id`), unless the server asserts one. */
  principalId: string
  displayName: string
  color: string
  agent: boolean
  cursor?: { x: number; y: number; visible: boolean; windowId?: string; shape?: CursorShapeName }
}

/** Used when the server assigned no color. */
export const PRESENCE_FALLBACK_COLOR = "#3b82f6"

function entry(p: PresenceParticipant): PresenceEntry {
  return {
    participantId: p.participantId,
    principalId: p.principalId,
    displayName: p.displayName,
    color: p.color || PRESENCE_FALLBACK_COLOR,
    agent: p.kind === "agent",
  }
}

/** Who is in a Space's presence and where their cursors are. Pure state. */
export class PresenceRoster {
  private readonly byId = new Map<string, PresenceEntry>()
  meId: string | undefined

  /** A roster seeded from a fresh session: `me` and the members at join. */
  static from(me: PresenceParticipant, members: readonly PresenceMember[] = []): PresenceRoster {
    const r = new PresenceRoster()
    r.meId = me.participantId
    r.byId.set(me.participantId, entry(me))
    for (const m of members) {
      const e = entry(m.participant)
      if (m.cursor) e.cursor = { x: m.cursor.x, y: m.cursor.y, visible: m.cursor.visible, ...(m.cursor.windowId ? { windowId: m.cursor.windowId } : {}) }
      if (!r.byId.has(e.participantId)) r.byId.set(e.participantId, e)
    }
    return r
  }

  /** Everyone, me first, then in join order. */
  get entries(): PresenceEntry[] {
    const all = [...this.byId.values()]
    return all.sort((a, b) => Number(b.participantId === this.meId) - Number(a.participantId === this.meId))
  }

  /**
   * The color an agent shows as, for its avatar and its cursor alike: the
   * color the server assigned once it is present (a requested color is kept
   * unless another participant already holds it), else its stable
   * {@link presenceColor}. Keyed by the id it joined with.
   */
  colorOf(principalId: string): string {
    for (const e of this.byId.values()) if (e.principalId === principalId) return e.color
    return presenceColor(principalId)
  }

  /** Everyone but me: the cursors to draw (mine is the real pointer). */
  get others(): PresenceEntry[] {
    return [...this.byId.values()].filter((e) => e.participantId !== this.meId)
  }

  get(participantId: string): PresenceEntry | undefined {
    return this.byId.get(participantId)
  }

  /** Folds one event in. Returns whether anything changed. */
  apply(e: PresenceEvent): boolean {
    switch (e.kind) {
      case "joined": {
        if (!e.participant || this.byId.has(e.participant.participantId)) return false
        this.byId.set(e.participant.participantId, entry(e.participant))
        return true
      }
      case "left":
        return e.participantId !== undefined && this.byId.delete(e.participantId)
      case "cursor_moved": {
        const who = e.participantId !== undefined ? this.byId.get(e.participantId) : undefined
        if (!who || !e.cursor) return false
        const keep = who.cursor?.shape
        const shape = e.cursor.shapeSource && e.cursor.shapeSource !== "unspecified" && isCursorShape(e.cursor.shape) ? e.cursor.shape : keep
        who.cursor = { x: e.cursor.x, y: e.cursor.y, visible: e.cursor.visible, ...(e.cursor.windowId ? { windowId: e.cursor.windowId } : {}), ...(shape ? { shape } : {}) }
        return true
      }
      case "shape_changed": {
        const who = e.participantId !== undefined ? this.byId.get(e.participantId) : undefined
        if (!who?.cursor || !isCursorShape(e.shape)) return false
        who.cursor.shape = e.shape
        return true
      }
      case "heartbeat": {
        const live = new Set(e.participantIds ?? [])
        let changed = false
        for (const id of [...this.byId.keys()]) {
          if (id !== this.meId && !live.has(id)) changed = this.byId.delete(id) || changed
        }
        return changed
      }
      default:
        return false
    }
  }
}

/**
 * Reads `session` until an event matches, folding every event into `roster`
 * when given. Throws after `timeoutMs` or `maxEvents` events.
 */
export async function waitForPresence(
  session: PresenceEventSource,
  match: (e: PresenceEvent) => boolean,
  timeoutMs: number,
  maxEvents = 50,
  roster?: PresenceRoster,
): Promise<PresenceEvent> {
  const deadline = Date.now() + timeoutMs
  for (let i = 0; i < maxEvents; i++) {
    const left = deadline - Date.now()
    if (left <= 0) break
    const e = await session.nextEvent(BigInt(left))
    if (!e) break
    roster?.apply(e)
    if (match(e)) return e
  }
  throw new Error(`presence: no matching event within ${timeoutMs} ms / ${maxEvents} events`)
}

// ---------------------------------------------------------------- netcode

/**
 * The presence netcode model, mirrored exactly from the Rust core
 * (`cua_spaces::presence::view`) and checked against the same conformance
 * vectors. Pure and deterministic: every method takes the local time in
 * milliseconds (`Date.now()`).
 *
 * - your own cursor is drawn at the local pointer, never from the network;
 * - remote cursors render {@link STREAM_DELAY_MS} (100 ms; 66 on the datagram
 *   channel) behind the newest sample, Catmull-Rom through at least 4
 *   samples, else linear;
 * - past the newest sample they extrapolate for at most 100 ms, then hold;
 * - a jump over 25% of the surface, a target change or a re-show snaps;
 * - a cursor idle for 5 s fades out over 300 ms;
 * - a participant a heartbeat does not list, or everyone when heartbeats
 *   stop for 3 intervals, is removed.
 */
export const STREAM_DELAY_MS = 100
/** Render delay on the 30 Hz datagram channel. */
export const DATAGRAM_DELAY_MS = 66
/** Longest extrapolation past the newest sample. */
export const EXTRAPOLATE_MS = 100
/** A jump longer than this (normalized distance) snaps. */
export const SNAP_DISTANCE = 0.25
/** A cursor idle this long starts fading. */
export const IDLE_FADE_AFTER_MS = 5_000
/** Fade duration. */
export const IDLE_FADE_MS = 300
/** Heartbeat interval the SDKs ask for. */
export const HEARTBEAT_INTERVAL_MS = 5_000
/** Missed heartbeats before everyone else is dropped. */
export const HEARTBEAT_MISSES = 3
const MAX_SAMPLES = 8

interface Sample {
  t: number
  x: number
  y: number
}

const clamp01 = (v: number) => Math.min(1, Math.max(0, v))

/** Opacity of a cursor idle for `idleMs`. */
export function idleAlpha(idleMs: number): number {
  if (idleMs <= IDLE_FADE_AFTER_MS) return 1
  return clamp01(1 - (idleMs - IDLE_FADE_AFTER_MS) / IDLE_FADE_MS)
}

/** One remote cursor's interpolation buffer. */
export class CursorSmoother {
  private samples: Sample[] = []
  private offset: number | undefined
  private target = ""
  private lastMoveMs = Number.NEGATIVE_INFINITY

  constructor(readonly delayMs: number = STREAM_DELAY_MS) {}

  /** Adds a sample (`serverMs` 0 = unknown: the local time is used). Out-of-order samples are dropped. */
  push(serverMs: number, localMs: number, x: number, y: number, target: string): void {
    const t = serverMs > 0 ? serverMs : localMs
    const candidate = localMs - t
    this.offset = this.offset !== undefined && this.offset <= candidate ? this.offset : candidate
    x = clamp01(x)
    y = clamp01(y)
    const last = this.samples[this.samples.length - 1]
    if (last) {
      if (t <= last.t) return
      const jump = Math.sqrt((x - last.x) ** 2 + (y - last.y) ** 2)
      if (target !== this.target || jump > SNAP_DISTANCE) this.samples = []
      if (x !== last.x || y !== last.y) this.lastMoveMs = localMs
    } else {
      this.lastMoveMs = localMs
    }
    this.target = target
    this.samples.push({ t, x, y })
    while (this.samples.length > MAX_SAMPLES) this.samples.shift()
  }

  /** Forgets every sample (the next one snaps). */
  reset(): void {
    this.samples = []
  }

  /** The position to draw at `localMs`, or undefined before any sample. */
  position(localMs: number): [number, number] | undefined {
    const s = this.samples
    const first = s[0]
    const last = s[s.length - 1]
    if (!first || !last) return undefined
    const render = localMs - (this.offset ?? 0) - this.delayMs
    if (render <= first.t) return [first.x, first.y]
    if (render >= last.t) {
      const dt = render - last.t
      if (dt > EXTRAPOLATE_MS || s.length < 2) return [last.x, last.y]
      const prev = s[s.length - 2] as Sample
      const span = last.t - prev.t
      if (span <= 0) return [last.x, last.y]
      const vx = (last.x - prev.x) / span
      const vy = (last.y - prev.y) / span
      return [clamp01(last.x + vx * dt), clamp01(last.y + vy * dt)]
    }
    let i = 0
    while (i + 1 < s.length && (s[i + 1] as Sample).t <= render) i++
    const a = s[i] as Sample
    const b = s[i + 1] as Sample
    const u = (render - a.t) / (b.t - a.t)
    if (s.length < 4) return [a.x + (b.x - a.x) * u, a.y + (b.y - a.y) * u]
    const p0 = i > 0 ? (s[i - 1] as Sample) : a
    const p3 = i + 2 < s.length ? (s[i + 2] as Sample) : b
    const cr = (q0: number, q1: number, q2: number, q3: number) => {
      const u2 = u * u
      const u3 = u2 * u
      return 0.5 * (2 * q1 + (-q0 + q2) * u + (2 * q0 - 5 * q1 + 4 * q2 - q3) * u2 + (-q0 + 3 * q1 - 3 * q2 + q3) * u3)
    }
    return [clamp01(cr(p0.x, a.x, b.x, p3.x)), clamp01(cr(p0.y, a.y, b.y, p3.y))]
  }

  /** Opacity at `localMs`. */
  alpha(localMs: number): number {
    return idleAlpha(localMs - this.lastMoveMs)
  }
}

/** One cursor to draw now. */
export interface PresenceDrawable {
  participantId: string
  displayName: string
  color: string
  isMe: boolean
  isAgent: boolean
  x: number
  y: number
  shape: CursorShapeName
  shapeSource: string
  alpha: number
  displayId: string
  windowId?: string
}

interface ViewMember {
  participant: PresenceParticipant
  cursor?: NonNullable<PresenceEvent["cursor"]>
  smoother: CursorSmoother
  shape: CursorShapeName
  shapeSource: string
}

const targetKey = (c: NonNullable<PresenceEvent["cursor"]>) => (c.windowId ? `w:${c.windowId}` : `d:${c.displayId ?? ""}`)

/** Who is present and what to draw for each, folded from presence events. */
export class PresenceView {
  private readonly members = new Map<string, ViewMember>()
  private order: string[] = []
  private heartbeatIntervalMs = HEARTBEAT_INTERVAL_MS
  private lastHeartbeatMs: number | undefined

  /** A view for `me` (the caller's participant id). */
  constructor(readonly me: string, readonly delayMs: number = STREAM_DELAY_MS) {}

  /** A view seeded from a fresh session: `me` and the members at join. */
  static from(me: PresenceParticipant, members: readonly PresenceMember[] = [], delayMs: number = STREAM_DELAY_MS, localMs: number = Date.now()): PresenceView {
    const v = new PresenceView(me.participantId, delayMs)
    v.upsert(me)
    for (const m of members) {
      v.upsert(m.participant)
      if (m.cursor) v.apply({ kind: "cursor_moved", participantId: m.participant.participantId, cursor: m.cursor } as PresenceEvent, localMs)
    }
    return v
  }

  /** The expected heartbeat interval (default 5 s). */
  setHeartbeatIntervalMs(ms: number): void {
    if (ms > 0) this.heartbeatIntervalMs = ms
  }

  /** Adds or updates a participant. */
  upsert(participant: PresenceParticipant): void {
    const m = this.members.get(participant.participantId)
    if (m) {
      m.participant = participant
      return
    }
    this.order.push(participant.participantId)
    this.members.set(participant.participantId, { participant, smoother: new CursorSmoother(this.delayMs), shape: "arrow", shapeSource: "unspecified" })
  }

  private remove(id: string): boolean {
    if (id === this.me) return false
    this.order = this.order.filter((o) => o !== id)
    return this.members.delete(id)
  }

  /** Participant ids, the caller first, then in join order. */
  participantIds(): string[] {
    return [...this.order.filter((id) => id === this.me), ...this.order.filter((id) => id !== this.me)]
  }

  participant(id: string): PresenceParticipant | undefined {
    return this.members.get(id)?.participant
  }

  /** A participant's current shape (the caller's included). */
  shapeOf(id: string): CursorShapeName | undefined {
    return this.members.get(id)?.shape
  }

  /** Folds one event in at `localMs`. Returns whether anything changed. */
  apply(e: PresenceEvent, localMs: number): boolean {
    switch (e.kind) {
      case "joined": {
        if (!e.participant) return false
        const known = this.members.has(e.participant.participantId)
        this.upsert(e.participant)
        return !known
      }
      case "left":
        return e.participantId !== undefined && this.remove(e.participantId)
      case "cursor_moved": {
        const m = e.participantId !== undefined ? this.members.get(e.participantId) : undefined
        const c = e.cursor
        if (!m || !c) return false
        const received = c.receivedMs && c.receivedMs > 0 ? c.receivedMs : localMs
        const wasVisible = m.cursor?.visible === true
        if (c.visible) {
          if (!wasVisible) m.smoother.reset()
          m.smoother.push(c.atMs ?? 0, received, c.x, c.y, targetKey(c))
        }
        if (c.shapeSource && c.shapeSource !== "unspecified") {
          m.shape = isCursorShape(c.shape) ? c.shape : "arrow"
          m.shapeSource = c.shapeSource
        }
        m.cursor = c
        return true
      }
      case "shape_changed": {
        const m = e.participantId !== undefined ? this.members.get(e.participantId) : undefined
        if (!m) return false
        const shape: CursorShapeName = isCursorShape(e.shape) ? e.shape : "arrow"
        const source = e.shapeSource || "unspecified"
        if (m.shape === shape && m.shapeSource === source) return false
        m.shape = shape
        m.shapeSource = source
        return true
      }
      case "heartbeat": {
        this.lastHeartbeatMs = localMs
        const live = new Set(e.participantIds ?? [])
        let changed = false
        for (const id of this.order.filter((o) => o !== this.me && !live.has(o))) changed = this.remove(id) || changed
        return changed
      }
      default:
        return false
    }
  }

  /** Drops everyone but the caller when heartbeats stopped for 3 intervals. Returns the removed ids. */
  expire(localMs: number): string[] {
    if (this.lastHeartbeatMs === undefined) return []
    if (localMs - this.lastHeartbeatMs <= HEARTBEAT_MISSES * this.heartbeatIntervalMs) return []
    const gone = this.order.filter((id) => id !== this.me)
    for (const id of gone) this.remove(id)
    return gone
  }

  /**
   * Everything to draw at `localMs`: remote cursors interpolated and faded,
   * and your own cursor at `pointer` (normalized; undefined when the pointer
   * is not over the surface) with its server shape. Also applies {@link expire}.
   */
  drawables(localMs: number, pointer?: { x: number; y: number }): PresenceDrawable[] {
    this.expire(localMs)
    const out: PresenceDrawable[] = []
    for (const id of this.participantIds()) {
      const m = this.members.get(id)
      if (!m) continue
      const isMe = id === this.me
      let x: number
      let y: number
      let alpha = 1
      if (isMe) {
        if (!pointer) continue
        x = pointer.x
        y = pointer.y
      } else {
        if (!m.cursor?.visible) continue
        const p = m.smoother.position(localMs)
        if (!p) continue
        alpha = m.smoother.alpha(localMs)
        if (alpha <= 0) continue
        ;[x, y] = p
      }
      out.push({
        participantId: id,
        displayName: m.participant.displayName,
        color: m.participant.color || PRESENCE_FALLBACK_COLOR,
        isMe,
        isAgent: m.participant.kind === "agent",
        x,
        y,
        shape: m.shape,
        shapeSource: m.shapeSource,
        alpha,
        displayId: m.cursor?.displayId ?? "",
        ...(m.cursor?.windowId ? { windowId: m.cursor.windowId } : {}),
      })
    }
    return out
  }
}
