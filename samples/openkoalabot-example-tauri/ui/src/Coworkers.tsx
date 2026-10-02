// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Routines, group chats and the other participants' presence cursors. The
// model is the SDK's (`cua_spaces::routines` / `::groups` in the core,
// `@trycua/cua/spaces/routines` / `groups` for the labels and bounds here);
// these views only render it and call the shell's commands.
import { useEffect, useRef, useState } from "react"
import { GROUP_MAX_BOTS, GROUP_MIN_BOTS, canCreateGroup } from "@trycua/cua/spaces/groups"
import { cursorArt, cursorArtSvg, presenceColor, presenceTextColor } from "@trycua/cua/spaces/presence"
import { scheduleLabel, type RoutineSchedule } from "@trycua/cua/spaces/routines"
import type { Avatar, GroupView, RoutineRow, RoutinesView } from "./api"
import koalaMark from "./assets/koala-mark.svg"
import * as I from "./icons"

export interface NamedBot {
  id: string
  name: string
}

let assigned: Record<string, string> = {}

/**
 * The colors the server assigned to whoever is present, by the id each joined
 * with, from the core's presence avatars (the SDK roster).
 */
export function assignedColorsFrom(avatars: ReadonlyArray<Pick<Avatar, "principal_id" | "color">>): Record<string, string> {
  const out: Record<string, string> = {}
  for (const a of avatars) if (a.principal_id && a.color) out[a.principal_id] = a.color.toLowerCase()
  return out
}

/** Replaces the assigned colors; returns whether any changed. */
export function setAssignedColors(colors: Record<string, string>): boolean {
  const before = JSON.stringify(Object.entries(assigned).sort())
  assigned = { ...colors }
  return before !== JSON.stringify(Object.entries(assigned).sort())
}

/**
 * A Bot's avatar color, the same as its cursor: while the Bot is present on
 * the Space, the color the server assigned it (a requested color is kept
 * unless another participant already holds it); before that, its stable
 * presence color from the SDK, the one it requests when it joins.
 */
export function botAvatarColor(botID: string): string {
  return assigned[botID] ?? presenceColor(botID)
}

/**
 * A Bot's one-line sidebar preview: the last line of its own thread, or, when
 * that thread has nothing yet, its latest reply in any group chat.
 */
export function botPreview(botID: string, ownLast: string, groups: ReadonlyArray<GroupView>): string {
  if (ownLast) return ownLast
  let best: { at: string; text: string } | undefined
  for (const g of groups) {
    for (const m of g.messages) {
      if (m.speaker.kind !== "bot" || m.speaker.botID !== botID || m.undelivered || !m.text.trim()) continue
      if (!best || m.at >= best.at) best = { at: m.at, text: `${g.title}: ${m.text.trim()}` }
    }
  }
  return best?.text ?? ""
}

/** A Bot's koala on its presence color. */
export function BotAvatar({ botId, size = "" }: { botId: string; size?: "" | "sm" | "xs" | "lg" }) {
  return (
    <span className={`avatar ${size}`} data-bot={botId} style={{ background: botAvatarColor(botId) }}>
      <img src={koalaMark} alt="" />
    </span>
  )
}

// ------------------------------------------------------------- routines

const WEEKDAYS = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"]

/** The create form's schedule from its fields; `undefined` when invalid. */
export function scheduleFrom(kind: RoutineSchedule["kind"], minutes: number, time: string, weekday: number): RoutineSchedule | undefined {
  const [h, m] = time.split(":").map(Number)
  const clock = Number.isInteger(h) && Number.isInteger(m) && h! >= 0 && h! < 24 && m! >= 0 && m! < 60
  if (kind === "everyMinutes") return Number.isInteger(minutes) && minutes > 0 ? { kind, minutes } : undefined
  if (!clock) return undefined
  if (kind === "dailyAt") return { kind, hour: h!, minute: m! }
  return { kind, weekday, hour: h!, minute: m! }
}

/** A Bot's routines: the list (enable, Run now, delete) and a compact create form. */
export function RoutinesPanel(p: {
  bot: NamedBot
  view: RoutinesView
  onCreate: (title: string, prompt: string, schedule: RoutineSchedule) => void
  onEnable: (id: string, enabled: boolean) => void
  onRun: (id: string) => void
  onDelete: (id: string) => void
}) {
  const mine = p.view.routines.filter((r) => r.botID === p.bot.id)
  const [title, setTitle] = useState("")
  const [prompt, setPrompt] = useState("")
  const [kind, setKind] = useState<RoutineSchedule["kind"]>("dailyAt")
  const [minutes, setMinutes] = useState(60)
  const [time, setTime] = useState("09:00")
  const [weekday, setWeekday] = useState(2)
  const schedule = scheduleFrom(kind, minutes, time, weekday)
  const ok = !!schedule && title.trim() !== "" && prompt.trim() !== ""
  return (
    <div className="scroll">
      <div className="col routines">
        <div className="group-list">
          {mine.length === 0 && <div className="list-row muted">No routines yet</div>}
          {mine.map((r) => (
            <RoutineLine key={r.id} r={r} onEnable={p.onEnable} onRun={p.onRun} onDelete={p.onDelete} />
          ))}
        </div>
        <div className="group-list form">
          <input className="input" placeholder="Title" value={title} onChange={(e) => setTitle(e.target.value)} aria-label="Title" />
          <textarea className="input" rows={2} placeholder={`What ${p.bot.name} does each time`} value={prompt} onChange={(e) => setPrompt(e.target.value)} aria-label="Prompt" />
          <div className="form-row">
            <select className="select" value={kind} onChange={(e) => setKind(e.target.value as RoutineSchedule["kind"])} aria-label="Repeat">
              <option value="everyMinutes">Every</option>
              <option value="dailyAt">Daily at</option>
              <option value="weeklyOn">Weekly on</option>
            </select>
            {kind === "everyMinutes" && (
              <label className="inline">
                <input className="input num" type="number" min={1} value={minutes} onChange={(e) => setMinutes(Number(e.target.value))} aria-label="Minutes" /> minutes
              </label>
            )}
            {kind === "weeklyOn" && (
              <select className="select" value={weekday} onChange={(e) => setWeekday(Number(e.target.value))} aria-label="Weekday">
                {WEEKDAYS.map((d, i) => (
                  <option key={d} value={i + 1}>
                    {d}
                  </option>
                ))}
              </select>
            )}
            {kind !== "everyMinutes" && <input className="input time" type="time" value={time} onChange={(e) => setTime(e.target.value)} aria-label="Time" />}
            <span className="spacer" />
            <button
              className="btn btn-primary"
              disabled={!ok}
              onClick={() => {
                if (!schedule) return
                p.onCreate(title.trim(), prompt.trim(), schedule)
                setTitle("")
                setPrompt("")
              }}
            >
              Add routine
            </button>
          </div>
        </div>
      </div>
    </div>
  )
}

function RoutineLine({ r, onEnable, onRun, onDelete }: { r: RoutineRow; onEnable: (id: string, on: boolean) => void; onRun: (id: string) => void; onDelete: (id: string) => void }) {
  return (
    <div className={`list-row ${r.isEnabled ? "" : "disabled"}`} title={r.lastOutcome ?? "Not run yet"}>
      <input type="checkbox" checked={r.isEnabled} onChange={(e) => onEnable(r.id, e.target.checked)} aria-label={`${r.title} enabled`} />
      <span className="grow">
        <b>{r.title}</b> <span className="muted">{scheduleLabel(r.schedule)}</span>
      </span>
      {r.lastOutcome && <span className="muted outcome">{r.lastOutcome}</span>}
      <button className="btn btn-ghost" onClick={() => onRun(r.id)}>
        Run now
      </button>
      <button className="icon-btn" title="Delete" onClick={() => onDelete(r.id)}>
        <I.Close />
      </button>
    </div>
  )
}

// ---------------------------------------------------------- group chats

/** New group chat: a title and 2 to 6 Bots; Create is disabled outside the bound. */
export function NewGroupSheet({ bots, onCancel, onCreate }: { bots: NamedBot[]; onCancel: () => void; onCreate: (title: string, members: string[]) => void }) {
  const [title, setTitle] = useState("")
  const [picked, setPicked] = useState<string[]>([])
  const ok = canCreateGroup(picked) && title.trim() !== ""
  return (
    <div className="scrim" role="dialog" aria-modal="true">
      <div className="sheet sm">
        <div className="sheet-head">
          <h2>New group chat</h2>
        </div>
        <div className="sheet-body">
          <label className="field">
            <span>Title</span>
            <input className="input" autoFocus value={title} onChange={(e) => setTitle(e.target.value)} />
          </label>
          <div className="field">
            <span>
              Bots ({picked.length} of {GROUP_MAX_BOTS})
            </span>
            <div className="group-list">
              {bots.map((b) => {
                const on = picked.includes(b.id)
                return (
                  <label key={b.id} className={`list-row ${!on && picked.length >= GROUP_MAX_BOTS ? "disabled" : ""}`}>
                    <input
                      type="checkbox"
                      checked={on}
                      disabled={!on && picked.length >= GROUP_MAX_BOTS}
                      onChange={() => setPicked((l) => (on ? l.filter((x) => x !== b.id) : [...l, b.id]))}
                    />
                    <BotAvatar botId={b.id} size="xs" />
                    <span className="grow">{b.name}</span>
                  </label>
                )
              })}
            </div>
          </div>
        </div>
        <div className="sheet-foot">
          <span className="muted">
            {GROUP_MIN_BOTS} to {GROUP_MAX_BOTS} Bots
          </span>
          <span className="spacer" />
          <button className="btn" onClick={onCancel}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!ok} onClick={() => onCreate(title.trim(), picked)}>
            Create
          </button>
        </div>
      </div>
    </div>
  )
}

/** One group thread: the attributed transcript, the typing row and a composer. */
export function GroupThread({ chat, name, onSend }: { chat: GroupView; name: (botID: string) => string; onSend: (text: string) => void }) {
  const [text, setText] = useState("")
  const end = useRef<HTMLDivElement>(null)
  useEffect(() => end.current?.scrollIntoView({ block: "end" }), [chat.messages.length])
  const submit = () => {
    const t = text.trim()
    if (!t) return
    setText("")
    onSend(t)
  }
  return (
    <>
      <div className="scroll">
        <div className="col group-transcript">
          {chat.messages.map((m) => (
            <div key={m.id} className={`group-line ${m.speaker.kind} ${m.undelivered ? "undelivered" : ""}`}>
              {m.speaker.kind === "human" ? (
                <div className="bubble">{m.text}</div>
              ) : m.speaker.kind === "system" ? (
                <span className="muted">{m.text}</span>
              ) : (
                <>
                  <b className="who">
                    <BotAvatar botId={m.speaker.botID} size="xs" />
                    {name(m.speaker.botID)}
                  </b>
                  <span className="text">{m.text}</span>
                </>
              )}
            </div>
          ))}
          {chat.working.length > 0 && (
            <div className="group-line system">
              <span className="spinner" /> <span className="muted">{chat.working.map(name).join(", ")} working</span>
            </div>
          )}
          <div ref={end} />
        </div>
      </div>
      <div className="dock">
        <div className="composer">
          <textarea
            rows={1}
            value={text}
            placeholder={`Message ${chat.title}…`}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault()
                submit()
              }
            }}
          />
          <div className="tools">
            <span className="spacer" />
            <button className="send" title="Send" disabled={!text.trim()} onClick={submit}>
              <I.ArrowUp />
            </button>
          </div>
        </div>
      </div>
    </>
  )
}

// ------------------------------------------------------------- presence

/** Where a canvas's picture sits inside its box (`object-fit: contain`). */
export function contentRect(canvas: { width: number; height: number }, box: { left: number; top: number; width: number; height: number }) {
  if (!canvas.width || !canvas.height || !box.width || !box.height) return box
  const scale = Math.min(box.width / canvas.width, box.height / canvas.height)
  const width = canvas.width * scale
  const height = canvas.height * scale
  return { left: box.left + (box.width - width) / 2, top: box.top + (box.height - height) / 2, width, height }
}

/** Drawn size of a presence cursor (the shared art is a 32 px canvas). */
const CURSOR_PX = 20

/**
 * The other participants' cursors over the stream (never mine: that is the
 * real pointer), where the SDK's PresenceView draws them: the guest's cursor
 * shape in the shared art, the participant's color, and the idle fade.
 */
export function RemoteCursors({ avatars, canvas }: { avatars: Avatar[]; canvas: HTMLCanvasElement | null }) {
  const box = canvas ? { left: 0, top: 0, width: canvas.clientWidth, height: canvas.clientHeight } : null
  if (!box) return null
  const r = contentRect(canvas!, box)
  const k = CURSOR_PX / 32
  return (
    <div className="cursors" aria-hidden>
      {avatars
        .filter((a) => !a.me && a.cursor && (a.alpha ?? 1) > 0)
        .map((a) => {
          const color = a.color || "#3b82f6"
          const shape = a.shape ?? "arrow"
          const [hx, hy] = cursorArt(shape).hotspot
          return (
            <span
              key={a.participant_id}
              className="cursor"
              data-shape={shape}
              style={{
                left: r.left + a.cursor![0] * r.width - hx * k,
                top: r.top + a.cursor![1] * r.height - hy * k,
                opacity: a.alpha ?? 1,
                color,
              }}
            >
              <span className="cursor-art" style={{ width: CURSOR_PX, height: CURSOR_PX }} dangerouslySetInnerHTML={{ __html: cursorArtSvg(shape, color, CURSOR_PX) }} />
              <span className="cursor-name" style={{ background: color, color: presenceTextColor(color) }}>
                {a.display_name}
              </span>
            </span>
          )
        })}
    </div>
  )
}
