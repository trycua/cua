// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react"
import { getCurrentWebview } from "@tauri-apps/api/webview"
import { getCurrentWindow } from "@tauri-apps/api/window"
import { api, type Avatar, type SpaceInfo, type ThreadView, type Walkthrough, type WindowRow } from "./api"
import { ActivityGroup } from "./Activity"
import { awaitingApproval, lastLine, statusLabel, toBlocks, type Block, type FileEvent } from "./blocks"
import { APP_TELEPORT_IN_CUA_SPACES, decide, formatBytes, friendlyError, initials, normalizeSpaceUrl, type TeleportManifest } from "./logic"
import { attach, type StreamStatus } from "./rcdp"
import { Wizard } from "./Wizard"
import { SpaceSection } from "./SpaceSection"
import { invoke } from "@tauri-apps/api/core"
import { idleDropZone, sendingLabel, parseDrop, type DropZoneFile, type DropZoneStatus } from "@trycua/cua/teleport"
import { windowSource, type PipSource } from "@trycua/cua/spaces/pip"
import { DropZone, dropHits } from "./DropZone"
import { desktopSource, pipOpenSize, usePip, WindowList } from "./Pip"
import * as I from "./icons"
import {
  BotAvatar,
  GroupThread,
  NewGroupSheet,
  RemoteCursors,
  RoutinesPanel,
  assignedColorsFrom,
  botPreview,
  contentRect,
  setAssignedColors,
} from "./Coworkers"
import type { GroupView, RoutinesView } from "./api"
import { GROUP_MIN_BOTS } from "@trycua/cua/spaces/groups"
import koalaMark from "./assets/koala-mark.svg"
import koalaPeek from "./assets/koala-peek.svg"
import koalaSleep from "./assets/koala-sleep.svg"

// ------------------------------------------------------------------ model

interface Bot {
  id: string
  name: string
  agent: string
  space: string
}

const AGENTS = [
  { id: "claude-code", label: "Claude Code" },
  { id: "openai-codex", label: "Codex" },
]
const SETTLED = new Set(["idle", "finished", "failed", "crashed", "stopped", "awaiting_input"])
const isMac = typeof navigator !== "undefined" && /Mac/.test(navigator.userAgent)

// The page's saved state lives in the app's data directory, not in web
// storage (the webview keeps none on macOS): the shell injects it as
// `window.__OKB_UI_STATE__` before the page runs, and `ui_state_set` saves
// each key back (src-tauri/core/src/ui_state.rs).
const saved: Record<string, unknown> =
  (typeof window !== "undefined" && (window as { __OKB_UI_STATE__?: Record<string, unknown> }).__OKB_UI_STATE__) || {}

function load<T>(key: string, fallback: T): T {
  return key in saved ? (saved[key] as T) : fallback
}
function save(key: string, value: unknown): void {
  saved[key] = value
  api.uiStateSet(key, value ?? null).catch(() => {
    /* saved state is a convenience */
  })
}

type Run = <T>(p: Promise<T>) => Promise<T | undefined>

function useInterval(fn: () => void, ms: number, on: boolean) {
  const ref = useRef(fn)
  ref.current = fn
  useEffect(() => {
    if (!on) return
    const t = setInterval(() => ref.current(), ms)
    return () => clearInterval(t)
  }, [ms, on])
}

// -------------------------------------------------------------------- app

export function App() {
  const [spaces, setSpaces] = useState<SpaceInfo[]>([])
  const [spaceId, setSpaceId] = useState<string | null>(() => load("okb.space", null))
  const [bots, setBots] = useState<Bot[]>(() => load("okb.bots", []))
  const [botId, setBotId] = useState<string | null>(() => load("okb.bot", null))
  const [threads, setThreads] = useState<Record<string, ThreadView>>(() => load("okb.threads", {}))
  const [files, setFiles] = useState<Record<string, FileEvent[]>>(() => load("okb.files", {}))
  const [cloud, setCloud] = useState(false)
  // Bumped when the server-assigned presence colors change (avatars redraw).
  const [, setColorEpoch] = useState(0)
  const onPresenceColors = useCallback(() => setColorEpoch((n) => n + 1), [])
  const [error, setError] = useState("")
  const [notice, setNotice] = useState("")
  const [sheet, setSheet] = useState<null | "wizard" | "address" | "bot" | "teleport" | "group">(null)
  // What the middle pane shows: the selected Bot's thread or its routines, or a group chat.
  const [pane, setPane] = useState<{ kind: "thread" } | { kind: "routines" } | { kind: "group"; id: string }>({ kind: "thread" })
  const [routines, setRoutines] = useState<RoutinesView>({ routines: [], log: [] })
  const [groups, setGroups] = useState<GroupView[]>([])
  // App teleport ("Teleport an app...", dropped app bundles) ships with Cua
  // Spaces; this sample says so instead.
  const openTeleportApp = useCallback(() => setNotice(APP_TELEPORT_IN_CUA_SPACES), [])
  const [computerOpen, setComputerOpen] = useState(() => load("okb.computer", true))
  const [query, setQuery] = useState("")
  const [wizardAt, setWizardAt] = useState<Walkthrough["wizard"]>()
  const [autoPip, setAutoPip] = useState<string>()
  const [expand, setExpand] = useState<string>()

  useEffect(() => save("okb.space", spaceId), [spaceId])
  useEffect(() => save("okb.bots", bots), [bots])
  useEffect(() => save("okb.bot", botId), [botId])
  useEffect(() => save("okb.threads", threads), [threads])
  useEffect(() => save("okb.files", files), [files])
  useEffect(() => save("okb.computer", computerOpen), [computerOpen])
  // The core names and hires Bots for routines and group chats from this list.
  useEffect(() => {
    api.botsSync(bots).catch(() => {})
  }, [bots])

  const run: Run = useCallback(async (p) => {
    try {
      setError("")
      return await p
    } catch (e) {
      setError(String(e))
      return undefined
    }
  }, [])
  const refresh = useCallback(
    () =>
      run(api.listSpaces()).then((s) => {
        if (!s) return
        setSpaces(s)
        setSpaceId((cur) => (cur && s.some((x) => x.id === cur) ? cur : (s[0]?.id ?? null)))
      }),
    [run],
  )
  useEffect(() => {
    refresh()
    api.cloudConfigured().then(setCloud, () => setCloud(false))
  }, [refresh])

  // A scripted walkthrough (demos, screenshots): the same calls a person makes.
  useEffect(() => {
    let cancelled = false
    api.walkthrough().then(async (w) => {
      if (!w || cancelled) return
      // A walkthrough starts from a clean slate.
      setBots([])
      setThreads({})
      setFiles({})
      setBotId(null)
      if (w.theme) await getCurrentWindow().setTheme(w.theme).catch(() => {})
      let target: string | null = null
      for (const s of w.spaces ?? []) {
        const info = await run(api.addSpace(s.url, s.token ?? "", s.name ?? ""))
        if (info) target = info.id
      }
      await refresh()
      if (target) setSpaceId(target)
      const hired: Bot[] = []
      for (const b of w.bots ?? []) {
        if (!target) break
        const bot: Bot = { id: `${b.name.toLowerCase()}-${Math.random().toString(16).slice(2, 6)}`, name: b.name, agent: b.agent ?? AGENTS[0].id, space: target }
        hired.push(bot)
        setBots((l) => [bot, ...l.filter((x) => x.name !== bot.name)])
      }
      for (const [i, b] of (w.bots ?? []).entries()) {
        const bot = hired[i]
        if (!bot) break
        for (const text of b.say ?? []) {
          let t = await run(api.botSend(bot.space, bot.id, bot.agent, text))
          if (t) setThreads((m) => ({ ...m, [bot.id]: t! }))
          // The status in a send's answer is the last poll's; poll a few
          // times before trusting a settled state.
          for (let n = 0; n < 60 && t && (n < 4 || !SETTLED.has(t.status ?? "")); n++) {
            await new Promise((r) => setTimeout(r, 500))
            t = await api.botPoll(bot.id).catch(() => t)
            if (t) setThreads((m) => ({ ...m, [bot.id]: t! }))
          }
        }
      }
      const pick = hired.find((b) => b.name === w.select) ?? hired[0]
      if (pick) setBotId(pick.id)
      if (hired.length) await api.botsSync(hired).catch(() => {})
      const byName = (n: string) => hired.find((b) => b.name === n)
      if (w.routine && byName(w.routine.bot)) {
        const b = byName(w.routine.bot)!
        let v = await run(api.routineCreate(b.id, w.routine.title, w.routine.prompt, { kind: "everyMinutes", minutes: w.routine.minutes ?? 60 }))
        const made = v?.routines.at(-1)
        if (made && w.routine.run !== false) v = await run(api.routineRun(made.id))
        if (v) setRoutines(v)
        setBotId(b.id)
        setPane({ kind: "routines" })
      }
      if (w.group) {
        const ids = w.group.bots.map((n) => byName(n)?.id).filter((x): x is string => !!x)
        let g = await run(api.groupCreate(w.group.title, ids))
        const made = g?.at(-1)
        if (made && w.group.say) g = await run(api.groupSend(made.id, w.group.say))
        if (g) setGroups(g)
        if (made) setPane({ kind: "group", id: made.id })
      }
      if (w.computer !== undefined) setComputerOpen(w.computer)
      if (w.wizard) setWizardAt(w.wizard)
      if (w.pip) setAutoPip(w.pip)
      if (w.expand) setExpand(w.expand)
      if (w.sheet) setSheet(w.sheet)
    })
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Routines fire on the core's clock; the list shows their last outcome.
  useEffect(() => {
    api.routines().then(setRoutines, () => {})
    api.groups().then(setGroups, () => {})
  }, [])
  useInterval(() => api.routines().then(setRoutines, () => {}), 5000, true)
  const openGroup = pane.kind === "group" ? groups.find((g) => g.id === pane.id) : undefined
  useInterval(() => openGroup && api.groupCollect(openGroup.id).then(setGroups, () => {}), 1500, !!openGroup)
  const botName = (id: string) => bots.find((b) => b.id === id)?.name ?? id

  const space = spaces.find((s) => s.id === spaceId) ?? null
  const bot = bots.find((b) => b.id === botId) ?? null
  const thread = bot ? threads[bot.id] : undefined

  // One poll loop for every Bot whose turn is still running, or that was
  // just messaged (a send answers with the previous poll's status).
  const sentAt = useRef<Record<string, number>>({})
  const busy = bots.filter((b) => threads[b.id]?.run_id && (!SETTLED.has(threads[b.id]?.status ?? "") || Date.now() - (sentAt.current[b.id] ?? 0) < 5000))
  useInterval(
    () => {
      for (const b of busy) api.botPoll(b.id).then((t) => setThreads((m) => ({ ...m, [b.id]: t })), () => {})
    },
    1000,
    busy.length > 0,
  )

  const addFile = useCallback((id: string, turn: number, name: string, r: { guest_path: string; bytes: number; verified: boolean }) => {
    setFiles((m) => ({
      ...m,
      [id]: [...(m[id] ?? []), { turn, name, detail: `${formatBytes(r.bytes)} to ${r.guest_path}${r.verified ? ", SHA-256 verified" : ""}` }],
    }))
  }, [])

  const hire = (name: string, agent: string, firstTask?: string) => {
    if (!space) {
      setSheet("wizard")
      return
    }
    const b: Bot = { id: `${name.toLowerCase().replace(/[^a-z0-9]+/g, "-")}-${Math.random().toString(16).slice(2, 6)}`, name, agent, space: space.id }
    setBots((l) => [b, ...l])
    setBotId(b.id)
    if (firstTask) void send(b, firstTask)
  }

  const send = async (b: Bot, text: string) => {
    sentAt.current[b.id] = Date.now()
    const t = await run(api.botSend(b.space, b.id, b.agent, text))
    if (t) setThreads((m) => ({ ...m, [b.id]: t }))
    else api.botPoll(b.id).then((t) => setThreads((m) => ({ ...m, [b.id]: t })), () => {})
  }

  const filtered = bots.filter((b) => !query || b.name.toLowerCase().includes(query.toLowerCase()))

  return (
    <div className={`app ${computerOpen ? "" : "computer-closed"}`}>
      <Sidebar
        bots={filtered}
        total={bots.length}
        threads={threads}
        files={files}
        botId={pane.kind === "thread" ? botId : null}
        onBot={(id) => {
          setBotId(id)
          setPane({ kind: "thread" })
        }}
        routines={routines}
        onRoutines={(id) => {
          setBotId(id)
          setPane({ kind: "routines" })
        }}
        routinesOf={pane.kind === "routines" ? botId : null}
        groups={groups}
        groupId={pane.kind === "group" ? pane.id : null}
        onGroup={(id) => setPane({ kind: "group", id })}
        onNewGroup={bots.length >= GROUP_MIN_BOTS ? () => setSheet("group") : undefined}
        onNewBot={() => (space ? setSheet("bot") : setSheet("wizard"))}
        query={query}
        onQuery={setQuery}
        spaces={spaces}
        space={space}
        onSpace={setSpaceId}
        onNewSpace={() => setSheet("wizard")}
        onDelete={async (id) => {
          if (await run(api.deleteSpace(id))) {
            setSpaceId(null)
            refresh()
          }
        }}
        cloud={cloud}
      />
      <main className="thread">
        <header className="pane-header" data-tauri-drag-region>
          {openGroup ? (
            <>
              <span className="title">{openGroup.title}</span>
              <span className="subtitle">{openGroup.label}</span>
            </>
          ) : bot && pane.kind === "routines" ? (
            <>
              <BotAvatar botId={bot.id} size="sm" />
              <span className="title">{bot.name}</span>
              <span className="subtitle">Routines</span>
            </>
          ) : bot ? (
            <>
              <BotAvatar botId={bot.id} size="sm" />
              <span className="title">{bot.name}</span>
              <span className={`dot ${thread?.status ?? "none"}`} />
              <span className="subtitle">
                {statusLabel(thread?.status)} · {AGENTS.find((a) => a.id === bot.agent)?.label ?? bot.agent}
              </span>
            </>
          ) : (
            <span className="title">{space ? space.name : "OpenKoalaBots"}</span>
          )}
          <span className="spacer" data-tauri-drag-region />
          {bot && !openGroup && (
            <button
              className={`icon-btn ${pane.kind === "routines" ? "on" : ""}`}
              title={pane.kind === "routines" ? "Back to the thread" : "Routines"}
              aria-pressed={pane.kind === "routines"}
              onClick={() => setPane(pane.kind === "routines" ? { kind: "thread" } : { kind: "routines" })}
            >
              <I.Clock />
            </button>
          )}
          <button className={`icon-btn ${computerOpen ? "on" : ""}`} title={computerOpen ? "Hide the Computer" : "Show the Computer"} onClick={() => setComputerOpen(!computerOpen)}>
            <I.PanelRight />
          </button>
        </header>
        {error && (
          <div className="banner error">
            <I.Alert />
            <ErrorText error={error} />
            <button className="icon-btn" onClick={() => setError("")} title="Dismiss">
              <I.Close />
            </button>
          </div>
        )}
        {notice && (
          <div className="banner">
            <I.Check />
            <span className="selectable">{notice}</span>
            <button className="icon-btn" onClick={() => setNotice("")} title="Dismiss">
              <I.Close />
            </button>
          </div>
        )}
        {!space ? (
          <Empty art={koalaPeek} title="Give your Bots a computer" text="Bots work in a Space: a Linux, Windows or macOS desktop in the cloud or on this machine.">
            <div className="suggestions">
              <button className="btn btn-primary" onClick={() => setSheet("wizard")}>
                <I.Plus /> New Space
              </button>
              <button className="btn" onClick={() => setSheet("address")}>
                <I.Link /> Add by address
              </button>
            </div>
          </Empty>
        ) : openGroup ? (
          <GroupThread chat={openGroup} name={botName} onSend={(text) => void run(api.groupSend(openGroup.id, text)).then((g) => g && setGroups(g))} />
        ) : !bot ? (
          <HireEmpty onHire={hire} />
        ) : pane.kind === "routines" ? (
          <RoutinesPanel
            bot={bot}
            view={routines}
            onCreate={(title, prompt, schedule) => void run(api.routineCreate(bot.id, title, prompt, schedule)).then((v) => v && setRoutines(v))}
            onEnable={(id, on) => void run(api.routineEnable(id, on)).then((v) => v && setRoutines(v))}
            onRun={(id) => void run(api.routineRun(id)).then((v) => v && setRoutines(v))}
            onDelete={(id) => void run(api.routineDelete(id)).then((v) => v && setRoutines(v))}
          />
        ) : (
          <Thread
            bot={bot}
            thread={thread}
            expand={expand}
            files={files[bot.id] ?? []}
            onSend={(text) => send(bot, text)}
            onAgent={(agent) => setBots((l) => l.map((b) => (b.id === bot.id ? { ...b, agent } : b)))}
            onAttach={async (file) => {
              const bytes = new Uint8Array(await file.arrayBuffer())
              const r = await run(api.sendFileBytes(bot.space, file.name, bytes))
              if (r) addFile(bot.id, thread?.transcript.at(-1)?.turn ?? 0, file.name, r)
            }}
          />
        )}
      </main>
      <Computer
        space={space}
        run={run}
        onTeleportApp={openTeleportApp}
        autoPip={autoPip}
        onPresenceColors={onPresenceColors}
        sheetOpen={sheet !== null}
        onClose={() => setComputerOpen(false)}
        onSent={(name, r) => {
          if (bot && bot.space === space?.id) addFile(bot.id, thread?.transcript.at(-1)?.turn ?? 0, name, r)
          else setNotice(`Sent ${name}: ${formatBytes(r.bytes)} to ${r.guest_path}`)
        }}
      />
      {sheet === "wizard" && (
        <Wizard
          cloud={cloud}
          initial={wizardAt}
          onCancel={() => setSheet(null)}
          onAddByAddress={() => setSheet("address")}
          onCreate={async (plan, open) => {
            const info = await run(api.createSpace(plan))
            if (!info) return false
            await refresh()
            setSpaceId(info.id)
            if (open) setComputerOpen(true)
            setNotice(`${info.name} is ready.`)
            return true
          }}
        />
      )}
      {sheet === "address" && (
        <AddressSheet
          onCancel={() => setSheet(null)}
          onAdd={async (url, token, name) => {
            const info = await run(api.addSpace(url, token, name))
            if (!info) return
            await refresh()
            setSpaceId(info.id)
            setSheet(null)
          }}
        />
      )}
      {sheet === "bot" && (
        <NewBotSheet
          onCancel={() => setSheet(null)}
          onHire={(name, agent) => {
            hire(name, agent)
            setSheet(null)
          }}
        />
      )}
      {sheet === "group" && (
        <NewGroupSheet
          bots={bots}
          onCancel={() => setSheet(null)}
          onCreate={async (title, members) => {
            const g = await run(api.groupCreate(title, members))
            if (!g) return
            setGroups(g)
            const made = g.at(-1)
            if (made) setPane({ kind: "group", id: made.id })
            setSheet(null)
          }}
        />
      )}
      {sheet === "teleport" && space && <TeleportSheet space={space.id} run={run} onClose={() => setSheet(null)} setNotice={setNotice} />}
    </div>
  )
}

/** Opens a Cua link (Install Cua) through the shell, which allows only those. */
function openCuaLink(url: string): void {
  void invoke("open_cua_link", { url }).catch(() => {})
}

function ErrorText({ error }: { error: string }) {
  const f = friendlyError(error)
  return (
    <span className="selectable">
      {f.message}
      {f.link && (
        <button className="btn btn-primary" onClick={() => openCuaLink(f.link!.url)}>
          {f.link.label}
        </button>
      )}
      {f.detail && (
        <details>
          <summary>Details</summary>
          {f.detail}
        </details>
      )}
    </span>
  )
}

// ---------------------------------------------------------------- sidebar

function Sidebar(p: {
  bots: Bot[]
  total: number
  threads: Record<string, ThreadView>
  files: Record<string, FileEvent[]>
  botId: string | null
  onBot: (id: string) => void
  onNewBot: () => void
  query: string
  onQuery: (q: string) => void
  spaces: SpaceInfo[]
  space: SpaceInfo | null
  onSpace: (id: string) => void
  onNewSpace: () => void
  onDelete: (id: string) => void
  cloud: boolean
  routines: RoutinesView
  routinesOf: string | null
  onRoutines: (botId: string) => void
  groups: GroupView[]
  groupId: string | null
  onGroup: (id: string) => void
  /** Unset while there are fewer Bots than a group needs. */
  onNewGroup?: () => void
}) {
  const owners = p.bots.filter((b) => p.routines.routines.some((r) => r.botID === b.id))
  return (
    <aside className="sidebar">
      <div className={`brand ${isMac ? "traffic" : ""}`} data-tauri-drag-region>
        <img src={koalaMark} alt="" />
        <b>OpenKoalaBots</b>
      </div>
      <div className="sidebar-top">
        <button className="btn new-bot" onClick={p.onNewBot}>
          <I.Plus /> New Bot
        </button>
        <label className="search">
          <I.Search />
          <input placeholder="Search Bots" value={p.query} onChange={(e) => p.onQuery(e.target.value)} />
        </label>
      </div>
      <div className="section-label">Bots</div>
      <nav className="roster">
        {p.bots.map((b) => {
          const t = p.threads[b.id]
          const needsYou = t ? awaitingApproval(toBlocks(t.transcript)) : false
          return (
            <button key={b.id} className={`bot-row ${b.id === p.botId ? "on" : ""}`} onClick={() => p.onBot(b.id)}>
              <BotAvatar botId={b.id} />
              <span className="who">
                <span className="name">{b.name}</span>
                <span className="last">{botPreview(b.id, t ? lastLine(t.transcript, t.preview) : "", p.groups) || "No messages yet"}</span>
              </span>
              <span className="meta">
                {needsYou ? <span className="badge">1</span> : <span className={`dot ${t?.status ?? "none"}`} title={statusLabel(t?.status)} />}
              </span>
            </button>
          )
        })}
        {p.total === 0 && <div className="roster-empty">No Bots yet. Hire one to get started.</div>}
        {p.total > 0 && p.bots.length === 0 && <div className="roster-empty">No Bot matches “{p.query}”.</div>}
        <div className="section-label">Routines</div>
        {owners.length === 0 && <div className="roster-empty">No routines yet</div>}
        {owners.map((b) => {
          const n = p.routines.routines.filter((r) => r.botID === b.id).length
          return (
            <button key={b.id} className={`line-row ${p.routinesOf === b.id ? "on" : ""}`} onClick={() => p.onRoutines(b.id)}>
              <span className="grow">{b.name}</span>
              <span className="muted">{n}</span>
            </button>
          )
        })}
        <div className="section-label">
          Group chats
          <button className="icon-btn sm" title={p.onNewGroup ? "New group chat" : "A group needs at least 2 Bots"} disabled={!p.onNewGroup} onClick={p.onNewGroup}>
            <I.Plus />
          </button>
        </div>
        {p.groups.map((g) => (
          <button key={g.id} className={`line-row ${p.groupId === g.id ? "on" : ""}`} onClick={() => p.onGroup(g.id)}>
            <span className="grow">{g.title}</span>
            <span className="muted">{g.memberIDs.length}</span>
          </button>
        ))}
      </nav>
      <SpaceSection spaces={p.spaces} space={p.space} onSpace={p.onSpace} onNewSpace={p.onNewSpace} onDelete={p.onDelete} />
      <div className="account">
        <span className="avatar sm initials" style={{ background: "#5d5d63" }}>
          OP
        </span>
        <span className="who">
          <div className="name">Operator</div>
          <div className="sub">{p.cloud ? "Cua Cloud connected" : "This machine only"}</div>
        </span>
        <span className={`dot ${p.cloud ? "ready" : "none"}`} />
      </div>
    </aside>
  )
}

// ----------------------------------------------------------------- thread

function Empty({ art, title, text, children }: { art: string; title: string; text: string; children?: ReactNode }) {
  return (
    <div className="empty">
      <img className="art" src={art} alt="" />
      <h1>{title}</h1>
      <p>{text}</p>
      {children}
    </div>
  )
}

function Composer(p: {
  big?: boolean
  placeholder: string
  onSend: (text: string) => void
  agent: string
  onAgent?: (a: string) => void
  agentLocked?: boolean
  onAttach?: (f: File) => void
  disabled?: boolean
  lead?: ReactNode
}) {
  const [text, setText] = useState("")
  const fileInput = useRef<HTMLInputElement>(null)
  const submit = () => {
    const t = text.trim()
    if (!t || p.disabled) return
    setText("")
    p.onSend(t)
  }
  return (
    <div className={`composer ${p.big ? "big" : ""}`}>
      {p.lead}
      <textarea
        rows={p.big ? 2 : 1}
        value={text}
        placeholder={p.placeholder}
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault()
            submit()
          }
        }}
      />
      <div className="tools">
        <button className="icon-btn" title="Attach a file" disabled={!p.onAttach} onClick={() => fileInput.current?.click()}>
          <I.Paperclip />
        </button>
        <input
          ref={fileInput}
          type="file"
          hidden
          onChange={(e) => {
            const f = e.target.files?.[0]
            if (f) p.onAttach?.(f)
            e.target.value = ""
          }}
        />
        <select className="agent-picker" value={p.agent} disabled={p.agentLocked} onChange={(e) => p.onAgent?.(e.target.value)} title={p.agentLocked ? "The thread keeps its agent" : "Agent"}>
          {AGENTS.map((a) => (
            <option key={a.id} value={a.id}>
              {a.label}
            </option>
          ))}
        </select>
        <span className="spacer" />
        <button className="send" title="Send" disabled={!text.trim() || p.disabled} onClick={submit}>
          <I.ArrowUp />
        </button>
      </div>
    </div>
  )
}

function HireEmpty({ onHire }: { onHire: (name: string, agent: string, task: string) => void }) {
  const [name, setName] = useState("Koala")
  const [agent, setAgent] = useState(AGENTS[0].id)
  return (
    <Empty art={koalaPeek} title="Hire a Bot to get started" text="Name it and give it a first task. It works on its own computer, and you can watch or step in.">
      <Composer
        big
        agent={agent}
        onAgent={setAgent}
        placeholder="Describe the first task…"
        onSend={(task) => onHire(name.trim() || "Koala", agent, task)}
        lead={
          <label className="attach-chip" style={{ marginLeft: -6 }}>
            Name
            <input value={name} onChange={(e) => setName(e.target.value)} style={{ border: 0, background: "transparent", outline: "none", width: 120, fontWeight: 600, color: "var(--text)" }} />
          </label>
        }
      />
    </Empty>
  )
}

function Thread(p: { bot: Bot; thread?: ThreadView; expand?: string; files: FileEvent[]; onSend: (t: string) => void; onAgent: (a: string) => void; onAttach: (f: File) => void }) {
  const blocks = useMemo(() => toBlocks(p.thread?.transcript ?? [], p.files), [p.thread, p.files])
  const end = useRef<HTMLDivElement>(null)
  useEffect(() => end.current?.scrollIntoView({ block: "end" }), [blocks.length, p.thread?.transcript.length])
  const started = !!p.thread?.run_id
  const working = started && !SETTLED.has(p.thread?.status ?? "")
  if (blocks.length === 0)
    return (
      <Empty art={koalaSleep} title={`What should ${p.bot.name} work on?`} text="Give it a task. It keeps this thread, so follow-ups pick up where it left off.">
        <Composer big agent={p.bot.agent} onAgent={p.onAgent} placeholder={`Message ${p.bot.name}…`} onSend={p.onSend} onAttach={p.onAttach} />
      </Empty>
    )
  return (
    <>
      <div className="scroll">
        <div className="col">
          {blocks.map((b, i) => (
            <BlockView key={i} block={b} bot={p.bot} live={i === blocks.length - 1} expand={p.expand} onAnswer={p.onSend} />
          ))}
          {working && (
            <div className="msg msg-bot">
              <BotAvatar botId={p.bot.id} />
              <div className="body">
                <div className="card status">
                  <span className="spinner" /> <b>{p.bot.name}</b> is working in its Space…
                </div>
              </div>
            </div>
          )}
          <div ref={end} />
        </div>
      </div>
      <div className="dock">
        {p.thread && !p.thread.accepts_message && (
          <div className="note">
            <I.Info /> {p.bot.name} is mid-turn: a message now would be refused, not queued.
          </div>
        )}
        <Composer agent={p.bot.agent} agentLocked={started} placeholder={`Message ${p.bot.name}…`} onSend={p.onSend} onAttach={p.onAttach} disabled={p.thread ? !p.thread.accepts_message : false} />
      </div>
    </>
  )
}

function BlockView({ block, bot, live, expand, onAnswer }: { block: Block; bot: Bot; live: boolean; expand?: string; onAnswer: (t: string) => void }) {
  if (block.kind === "user")
    return (
      <div className="msg msg-user">
        <div className="bubble">{block.text}</div>
      </div>
    )
  return (
    <div className="msg msg-bot">
      <BotAvatar botId={bot.id} />
      <div className="body">
        <div className="author">{bot.name}</div>
        {block.items.map((it, i) => {
          switch (it.kind) {
            case "text":
              return (
                <div key={i} className="text">
                  {it.text}
                </div>
              )
            case "activity":
              return <ActivityGroup key={i} summary={it.summary} steps={it.steps} defaultOpen={!!expand && it.steps.some((s) => s.includes(expand))} />
            case "approval":
              return (
                <div key={i} className="card approval">
                  <div className="card-head">
                    <I.Alert /> {bot.name} needs your approval
                  </div>
                  <div className="card-body">{it.prompt}</div>
                  {live && i === block.items.length - 1 && (
                    <div className="actions">
                      <button className="btn btn-primary" onClick={() => onAnswer(it.options[0])}>
                        Approve
                      </button>
                      <button className="btn" onClick={() => onAnswer(it.options[1])}>
                        Deny
                      </button>
                    </div>
                  )}
                </div>
              )
            case "file":
              return (
                <div key={i} className="card file">
                  <span className="file-icon">
                    <I.FileIcon />
                  </span>
                  <span>
                    <div className="name">{it.name}</div>
                    <div className="sub">{it.detail}</div>
                  </span>
                </div>
              )
          }
        })}
      </div>
    </div>
  )
}

// --------------------------------------------------------------- computer

function Computer(p: {
  space: SpaceInfo | null
  run: Run
  /** "Teleport an app..." or a dropped app bundle. */
  onTeleportApp: () => void
  sheetOpen: boolean
  onClose: () => void
  onSent: (name: string, r: { guest_path: string; bytes: number; verified: boolean }) => void
  /** Walkthrough: pop out `desktop`, or the first window whose app matches. */
  autoPip?: string
  /** The server-assigned presence colors changed: Bot avatars redraw. */
  onPresenceColors?: () => void
}) {
  const canvas = useRef<HTMLCanvasElement>(null)
  const [status, setStatus] = useState<StreamStatus | null>(null)
  const [on, setOn] = useState(true)
  const [over, setOver] = useState(false)
  const [zoneStatus, setZoneStatus] = useState<DropZoneStatus>(idleDropZone)
  const [windows, setWindows] = useState<WindowRow[]>([])
  const [avatars, setAvatarsState] = useState<Avatar[]>([])
  // Bot avatars follow the colors the server assigned (their cursors).
  const setAvatars = useCallback(
    (a: Avatar[]) => {
      const changed = setAssignedColors(assignedColorsFrom(a))
      setAvatarsState(a)
      if (changed) p.onPresenceColors?.()
    },
    [p.onPresenceColors],
  )
  const zone = useRef<HTMLElement | null>(null)
  const fileInput = useRef<HTMLInputElement>(null)
  const spaceId = p.space?.id ?? null
  const streams = !!p.space && p.space.features.includes("desktop_stream")
  const windowStreams = !!p.space && p.space.features.includes("window_stream")
  const { run, onSent, onTeleportApp, sheetOpen } = p
  const pip = usePip(spaceId)

  useEffect(() => {
    setStatus(null)
    if (!spaceId || !on || !streams) return
    let close: (() => void) | undefined
    let cancelled = false
    run(api.openStream(spaceId)).then((t) => {
      if (t && !cancelled && canvas.current) close = attach(t.ws_url, canvas.current, setStatus)
    })
    return () => {
      cancelled = true
      close?.()
    }
  }, [spaceId, on, streams, run])

  // The window list, refreshed while the panel is open.
  useEffect(() => {
    setWindows([])
    if (!spaceId || !windowStreams) return
    let cancelled = false
    const load = () =>
      api.listWindows(spaceId).then(
        (w) => !cancelled && setWindows((cur) => (JSON.stringify(cur) === JSON.stringify(w) ? cur : w)),
        () => {},
      )
    void load()
    const t = setInterval(load, 5000)
    return () => {
      cancelled = true
      clearInterval(t)
    }
  }, [spaceId, windowStreams])

  const desktopSize = { width: status?.width || 16, height: status?.height || 10 }
  const togglePip = useCallback(
    (source: PipSource, w?: WindowRow) => {
      void run(pip.toggle(source, pipOpenSize(source, desktopSize, w), p.space?.name ?? ""))
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [pip, run, p.space?.name, desktopSize.width, desktopSize.height],
  )

  // Walkthrough: open the requested PiPs (`desktop`, or an app or title to
  // match in the window list; comma-separated) once there is something to show.
  const autoPipDone = useRef(new Set<string>())
  useEffect(() => {
    if (!p.autoPip || !status || status.frames === 0) return
    for (const want of p.autoPip.split(",").map((x) => x.trim().toLowerCase())) {
      if (!want || autoPipDone.current.has(want)) continue
      if (want === "desktop") {
        autoPipDone.current.add(want)
        togglePip(desktopSource)
        continue
      }
      const w = windows.find((x) => x.app.toLowerCase().includes(want) || x.title.toLowerCase().includes(want))
      if (w) {
        autoPipDone.current.add(want)
        togglePip(windowSource(w), w)
      }
    }
  }, [p.autoPip, status, windows, togglePip])

  const sendPaths = useCallback(
    async (paths: string[]) => {
      if (!spaceId || !paths.length) return
      setZoneStatus({ kind: "sending", label: sendingLabel(paths) })
      const sent: DropZoneFile[] = []
      for (const path of paths) {
        const name = path.split(/[\\/]/).pop() ?? path
        const r = await run(api.sendFile(spaceId, path))
        if (!r || !r.verified) {
          setZoneStatus({ kind: "failed", message: `${name} was not sent` })
          return
        }
        onSent(name, r)
        sent.push({ name, bytes: r.bytes, dest: r.guest_path })
      }
      setZoneStatus({ kind: "sent", files: sent })
    },
    [spaceId, run, onSent],
  )

  const sendPicked = async (files: File[]) => {
    if (!spaceId || !files.length) return
    setZoneStatus({ kind: "sending", label: sendingLabel(files.map((f) => f.name)) })
    const sent: DropZoneFile[] = []
    for (const f of files) {
      const r = await run(api.sendFileBytes(spaceId, f.name, new Uint8Array(await f.arrayBuffer())))
      if (!r || !r.verified) {
        setZoneStatus({ kind: "failed", message: `${f.name} was not sent` })
        return
      }
      onSent(f.name, r)
      sent.push({ name: f.name, bytes: r.bytes, dest: r.guest_path })
    }
    setZoneStatus({ kind: "sent", files: sent })
  }

  // Files and app bundles dropped on the zone (native webview drops).
  useEffect(() => {
    if (!spaceId) return
    const un = getCurrentWebview().onDragDropEvent(async (e) => {
      if (e.payload.type === "leave") return setOver(false)
      if (e.payload.type !== "over" && e.payload.type !== "drop") return
      const inside = dropHits(zone.current, e.payload.position, window.devicePixelRatio)
      if (e.payload.type === "over") return setOver(inside)
      setOver(false)
      if (!inside || sheetOpen) return
      // An app bundle from Finder or the Dock is app teleport, which ships
      // with Cua Spaces: say so and send nothing. Plain files upload.
      if (parseDrop(e.payload.paths).kind === "app") return onTeleportApp()
      await sendPaths(e.payload.paths)
    })
    return () => {
      un.then((f) => f())
    }
  }, [spaceId, run, sendPaths, onTeleportApp, sheetOpen])

  const last = useRef(0)
  const live = status && status.frames > 0
  const desktopPip = pip.isOpen(desktopSource)
  return (
    <aside className="computer">
      <header className="pane-header" data-tauri-drag-region>
        <I.Monitor />
        <span className="title">Computer</span>
        <span className="spacer" data-tauri-drag-region />
        <button className="icon-btn" title={on ? "Stop the stream" : "Start the stream"} disabled={!streams} onClick={() => setOn(!on)}>
          {on ? <I.Stop /> : <I.Play />}
        </button>
        <button
          className={`icon-btn ${desktopPip ? "on" : ""}`}
          title={desktopPip ? "Close picture in picture" : "Picture in picture"}
          aria-pressed={desktopPip}
          disabled={!streams}
          onClick={() => togglePip(desktopSource)}
        >
          <I.Pip />
        </button>
        <button className="icon-btn" title="Hide" onClick={p.onClose}>
          <I.Close />
        </button>
      </header>
      <div className="computer-body">
        <div className="screen">
          <canvas
            ref={canvas}
            onDoubleClick={() => streams && togglePip(desktopSource)}
            onMouseMove={(e) => {
              const now = performance.now()
              if (now - last.current < 100) return
              last.current = now
              const r = contentRect(e.currentTarget, e.currentTarget.getBoundingClientRect())
              const x = (e.clientX - r.left) / r.width
              const y = (e.clientY - r.top) / r.height
              if (x >= 0 && x <= 1 && y >= 0 && y <= 1) api.presenceCursor(x, y).catch(() => {})
            }}
          />
          {live && <RemoteCursors avatars={avatars} canvas={canvas.current} />}
          {live ? (
            <span className="live">
              <span className="dot" /> Live · {status.width}×{status.height}
            </span>
          ) : (
            <div className="overlay">
              <img src={koalaSleep} alt="" />
              <div>{!p.space ? "No Space yet" : !streams ? "This Space does not stream its desktop" : !on ? "Stream stopped" : status ? `Stream ${status.state}` : "Connecting to the desktop…"}</div>
            </div>
          )}
        </div>
        <div className="screen-bar">
          <span>{p.space ? p.space.name : "No Space"}</span>
          {status && <span>· {status.frames} frames</span>}
          <span className="spacer" />
          {p.space && p.space.features.includes("presence") && <Presence space={p.space.id} run={run} onAvatars={setAvatars} />}
        </div>
        {p.space && <WindowList windows={windows} isOpen={pip.isOpen} onPip={(w) => togglePip(windowSource(w), w)} />}
        {p.space && (
          <>
            <DropZone
              spaceId={p.space.id}
              status={zoneStatus}
              over={over}
              zoneRef={(el) => (zone.current = el)}
              onSendFile={() => fileInput.current?.click()}
              onTeleportApp={() => onTeleportApp()}
            />
            <input
              ref={fileInput}
              type="file"
              multiple
              hidden
              onChange={(e) => {
                const files = Array.from(e.target.files ?? [])
                e.target.value = ""
                void sendPicked(files)
              }}
            />
          </>
        )}
        {p.space && (
          <div className="panel-card">
            <h3>Space</h3>
            <dl className="kv">
              <dt>Name</dt>
              <dd>{p.space.name}</dd>
              <dt>Id</dt>
              <dd className="selectable" title={p.space.id}>
                {p.space.id}
              </dd>
              <dt>Provider</dt>
              <dd>{p.space.provider}</dd>
              <dt>spacesd</dt>
              <dd>{p.space.spacesd_version || "none"}</dd>
            </dl>
          </div>
        )}
      </div>
    </aside>
  )
}

function Presence({ space, run, onAvatars }: { space: string; run: Run; onAvatars: (a: Avatar[]) => void }) {
  const [avatars, setOwn] = useState<Avatar[]>([])
  const setAvatars = useCallback(
    (a: Avatar[]) => {
      setOwn(a)
      onAvatars(a)
    },
    [onAvatars],
  )
  useEffect(() => {
    run(api.presenceJoin(space, "Operator")).then((a) => a && setAvatars(a))
    return () => {
      api.presenceLeave().catch(() => {})
    }
  }, [space, run])
  useInterval(() => api.presencePump().then(setAvatars).catch(() => {}), 400, avatars.length > 0)
  return (
    <span className="presence">
      {avatars.map((a) => (
        <span key={a.participant_id} className="avatar initials" style={{ background: a.color || "#5d5d63" }} title={`${a.display_name}${a.agent ? " (agent)" : ""}${a.me ? " (you)" : ""}`}>
          {initials(a.display_name)}
        </span>
      ))}
    </span>
  )
}

// ----------------------------------------------------------------- sheets

function AddressSheet({ onCancel, onAdd }: { onCancel: () => void; onAdd: (url: string, token: string, name: string) => void }) {
  const [url, setUrl] = useState("")
  const [token, setToken] = useState("")
  const [name, setName] = useState("")
  const u = normalizeSpaceUrl(url)
  return (
    <div className="scrim" role="dialog" aria-modal="true">
      <div className="sheet sm">
        <div className="sheet-head">
          <h2>Add a Space by address</h2>
          <p>A machine that already runs cua-spacesd. Nothing is created.</p>
        </div>
        <div className="sheet-body">
          <label className="field">
            <span>Address</span>
            <input className="input" autoFocus placeholder="10.0.0.5:3211" value={url} onChange={(e) => setUrl(e.target.value)} />
          </label>
          <label className="field">
            <span>Token</span>
            <input className="input" type="password" placeholder="The spacesd token" value={token} onChange={(e) => setToken(e.target.value)} />
          </label>
          <label className="field">
            <span>Name</span>
            <input className="input" placeholder="Optional" value={name} onChange={(e) => setName(e.target.value)} />
          </label>
        </div>
        <div className="sheet-foot">
          <span className="spacer" />
          <button className="btn" onClick={onCancel}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!u} onClick={() => u && onAdd(u, token, name)}>
            Add Space
          </button>
        </div>
      </div>
    </div>
  )
}

function NewBotSheet({ onCancel, onHire }: { onCancel: () => void; onHire: (name: string, agent: string) => void }) {
  const [name, setName] = useState("")
  const [agent, setAgent] = useState(AGENTS[0].id)
  return (
    <div className="scrim" role="dialog" aria-modal="true">
      <div className="sheet sm">
        <div className="sheet-head">
          <h2>Hire a Bot</h2>
          <p>Give it a name. It gets one long-lived thread in the current Space.</p>
        </div>
        <div className="sheet-body">
          <label className="field">
            <span>Name</span>
            <input className="input" autoFocus placeholder="Research, Inbox, Koala…" value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === "Enter" && name.trim() && onHire(name.trim(), agent)} />
          </label>
          <label className="field">
            <span>Agent</span>
            <select className="select" value={agent} onChange={(e) => setAgent(e.target.value)}>
              {AGENTS.map((a) => (
                <option key={a.id} value={a.id}>
                  {a.label}
                </option>
              ))}
            </select>
          </label>
        </div>
        <div className="sheet-foot">
          <span className="spacer" />
          <button className="btn" onClick={onCancel}>
            Cancel
          </button>
          <button className="btn btn-primary" disabled={!name.trim()} onClick={() => onHire(name.trim(), agent)}>
            Hire
          </button>
        </div>
      </div>
    </div>
  )
}

function TeleportSheet({ space, run, onClose, setNotice }: { space: string; run: Run; onClose: () => void; setNotice: (s: string) => void }) {
  const [manifest, setManifest] = useState<TeleportManifest | null>(null)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [ack, setAck] = useState(false)
  useEffect(() => {
    run(api.teleportManifest("firefox")).then((m) => {
      if (!m) return onClose()
      setManifest(m)
      setSelected(new Set(m.items.filter((i) => i.is_checked_by_default).map((i) => i.relative_path)))
    })
  }, [run, onClose])
  if (!manifest) return null
  const { decision, reason } = decide(manifest, selected, ack)
  return (
    <div className="scrim" role="dialog" aria-modal="true">
      <div className="sheet">
        <div className="sheet-head">
          <h2>Teleport {manifest.display_name} into this Space?</h2>
          <p>This is exactly what would leave this machine. Nothing moves until you approve.</p>
        </div>
        <div className="sheet-body">
          <ul className="item-list">
            {manifest.items.map((i) => (
              <li key={i.relative_path}>
                <label>
                  <input
                    type="checkbox"
                    checked={selected.has(i.relative_path)}
                    onChange={(e) => {
                      const s = new Set(selected)
                      if (e.target.checked) s.add(i.relative_path)
                      else s.delete(i.relative_path)
                      setSelected(s)
                    }}
                  />
                  {i.label}
                  {i.is_sensitive && <span className="tag">Sensitive</span>}
                  <span className="size">{formatBytes(i.estimated_bytes)}</span>
                </label>
              </li>
            ))}
          </ul>
          <label className="check">
            <input type="checkbox" checked={ack} onChange={(e) => setAck(e.target.checked)} />
            <span>I understand cookies and logins will be copied into the Space.</span>
          </label>
          {reason && <div className="error-text">{reason}</div>}
        </div>
        <div className="sheet-foot">
          <span className="spacer" />
          <button className="btn" onClick={onClose}>
            Cancel
          </button>
          <button
            className="btn btn-primary"
            disabled={!decision}
            onClick={async () => {
              if (!decision) return
              const r = await run(api.teleportApp(space, manifest.app, decision))
              if (r) setNotice(`Teleported ${manifest.display_name}: ${r.imported.join(", ")} (${formatBytes(r.bundle_bytes)})`)
              onClose()
            }}
          >
            Approve and teleport
          </button>
        </div>
      </div>
    </div>
  )
}
