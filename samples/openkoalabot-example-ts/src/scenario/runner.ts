/**
 * Executes samples/openkoalabot-example-scenario/scenario.json headlessly through the
 * app model (`BotStore`, `BotThread`) and the SDK. No window is opened; every
 * home the SDK sees is a temp directory; the teleported profile is generated;
 * the agent CLI is the fixture fake.
 */
import type { CuaLike } from "@trycua/cua"
import { SpaceCreateOptions, type SpaceInfo, type SpaceLike, type SpacesLike } from "@trycua/cua/spaces"
import { GroupChatError, GroupChatStore } from "@trycua/cua/spaces/groups"
import { PresenceRoster, agentIdentity, presenceColor, waitForPresence } from "@trycua/cua/spaces/presence"
import { RoutineStore, type RoutineSchedule } from "@trycua/cua/spaces/routines"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { dirname, join, resolve } from "node:path"
import { BotStore, sleep, type AgentEndpoint, type BotThread } from "../core/app.js"
import { BotStoreGroupMessenger, BotStoreRoutineRunner, jsonStorage } from "../core/coworkers.js"
import { nativeDesktopStream, openRuntime, webClientProbe } from "../core/runtime.js"
import { teleportNeedsCuaSpaces } from "../core/teleport.js"
import { assignedColorsFrom, botAvatarColor, setAssignedColors } from "../ui/coworkers.js"
import {
  type Lane,
  type ProfileFixture,
  type Scenario,
  type ScenarioResult,
  type Step,
  type StepResult,
  Skip,
  newNonce,
  sha256Hex,
  shellQuote,
  substitute,
  writeProfile,
  xorshiftBytes,
} from "../core/spec.js"

export interface RunOptions {
  spec: Scenario
  specDir: string
  lane: Lane
  url?: string
  token?: string
  importRoot: string
  cloudImage?: string
  /** The scripted model endpoint (spec `model` plus its URL from the environment); unset: agent-turn steps skip. */
  model?: { url: string; name: string; keyVar: string }
  log?: (line: string) => void
}

interface Ctx {
  opts: RunOptions
  nonce: string
  marker: string
  vars: Record<string, string>
  tmp: string
  cua: CuaLike
  spaces: SpacesLike
  info?: SpaceInfo
  space?: SpaceLike
  store?: BotStore
  extra: CuaLike[]
}

const num = (step: Step, key: string, fallback: number): number =>
  typeof step[key] === "number" ? (step[key] as number) : fallback

async function sh(space: SpaceLike, command: string, timeoutMs = 120_000): Promise<string> {
  const out = await space.bash(command, BigInt(timeoutMs))
  if (out.exitCode !== 0 || out.timedOut) throw new Error(`${command}: ${out.rendered}`)
  return out.stdout
}

function need<T>(v: T | undefined, what: string): T {
  if (v === undefined) throw new Error(`${what} is not available (an earlier step failed)`)
  return v
}

// ------------------------------------------------------------ steps

async function open(ctx: Ctx, step: Step): Promise<string> {
  const modes = (step.modes ?? {}) as Record<string, string>
  const mode = modes[ctx.opts.lane] ?? "add"
  const name = substitute(String(step.name ?? "openkoalabot-example-scenario-{nonce}"), ctx.vars)
  if (mode === "create") {
    const image = need(ctx.opts.cloudImage, "OPENKOALABOTS_CLOUD_IMAGE")
    const created = await ctx.spaces.create(SpaceCreateOptions.create({ on: "cloud", image, name, wait: true }))
    ctx.info = need(created.space, `a ready Space for ${created.pendingId ?? name}`)
  } else {
    const url = need(ctx.opts.url, "OPENKOALABOTS_SCENARIO_URL")
    ctx.info = await ctx.spaces.add(url, ctx.opts.token, name)
  }
  ctx.space = await ctx.spaces.space(ctx.info.id)
  const m = ctx.opts.model
  const endpoint: AgentEndpoint | undefined = m ? { baseUrl: m.url, model: m.name, envFromHost: [m.keyVar] } : undefined
  ctx.store = new BotStore(ctx.space, endpoint)
  return `${mode} ${ctx.info.id} (spacesd ${ctx.info.spacesdVersion}; ${ctx.info.features.length} features)`
}

function requireFeature(ctx: Ctx, step: Step): SpaceLike {
  const space = need(ctx.space, "the Space")
  const feature = typeof step.requires === "string" ? step.requires : undefined
  if (feature && !space.supports(feature)) throw new Skip(`the Space does not report ${feature}`)
  return space
}

async function stream(ctx: Ctx, step: Step): Promise<string> {
  const space = requireFeature(ctx, step)
  const o = {
    maxFps: num(step, "maxFps", 5),
    maxDimension: num(step, "maxDimension", 800),
    pollMs: num(step, "pollMs", 100),
    maxPolls: num(step, "maxPolls", 300),
  }
  const r = await nativeDesktopStream(space, o)
  if (r.frames < num(step, "minFrames", 1)) throw new Error(`${r.frames} frames`)
  if (step.firstFrameKeyframe !== false && r.firstIsKey !== true) throw new Error("the first frame is not a keyframe")
  if (step.keyframeRequest !== false && r.keyframeRequests < 1) {
    throw new Error(`keyframe request not counted: ${JSON.stringify(r.stats, (_, v) => (typeof v === "bigint" ? Number(v) : v))}`)
  }
  let detail = `native: ${r.frames} frames (${r.keyframes} key, first key) ${r.width}x${r.height} ${r.codec}`
  // The web UI's wire path, in Node. Reported, not gating: it is the
  // browser's code path and the native session above is the step's contract.
  try {
    const probe = await webClientProbe(space, {
      maxFps: o.maxFps,
      maxDimension: o.maxDimension,
      timeoutMs: o.pollMs * o.maxPolls,
      url: ctx.opts.lane === "cloud" ? undefined : ctx.opts.url,
      token: ctx.opts.token,
    })
    const w = probe.state
    detail +=
      `; web client (${probe.via}, ${probe.client} socket, ticket via ${probe.ticketVia}): hello+session_opened=${w.handshakeOk}, first packet keyframe=${w.firstFrameKeyframe}` +
      `${w.firstFrameAnnexBKeyframe === null ? "" : ` (Annex B IDR+SPS+PPS=${w.firstFrameAnnexBKeyframe})`}` +
      `${w.codecString ? `, ${w.codecString}` : ""}`
    if (!w.handshakeOk || w.firstFrameKeyframe !== true) throw new Error(`web wire path: ${detail}`)
  } catch (e) {
    if (e instanceof Error && e.message.startsWith("web wire path")) throw e
    detail += `; web wire: not exercised (${e instanceof Error ? e.message : String(e)})`
  }
  return detail
}

async function agentThread(ctx: Ctx, step: Step): Promise<string> {
  const space = need(ctx.space, "the Space")
  const store = need(ctx.store, "the store")
  // Agent runs speak the Agent Client Protocol through the cua-agents runner,
  // so a fake `claude` shell script can no longer stand in for a model. Real
  // harness runs against a scripted mock provider: cua-agents' e2e_live.
  if (step.fakeCli) throw new Skip("the step's fake CLI predates ACP agent runs; real runs are covered by cua-agents' e2e_live")
  const fake = step.fakeCli as { source: string; guestPath: string }
  const script = readFileSync(resolve(ctx.opts.specDir, fake.source))
  const home = (await space.home()).replace(/\/$/, "")
  const guest = fake.guestPath.replace(/^~/, home)
  await sh(space, `mkdir -p ${shellQuote(dirname(guest))}`)
  await space.write(guest, script.buffer.slice(script.byteOffset, script.byteOffset + script.byteLength) as ArrayBuffer)
  await sh(space, `chmod +x ${shellQuote(guest)}`)

  const turns = step.turns as Array<{ prompt?: string; message?: string; expectOutput: string }>
  const pollMs = num(step, "pollMs", 200)
  const maxPolls = num(step, "maxPolls", 300)
  const first = turns[0]
  const bot = { id: `koala-${ctx.nonce}`, name: "Koala", agent: String(step.agent ?? "claude-code") }
  const thread = await store.hire(bot, substitute(need(first.prompt, "turns[0].prompt"), ctx.vars))
  const notes: string[] = []
  try {
    for (let i = 0; i < turns.length; i++) {
      const t = turns[i]
      if (i > 0) {
        const d = await thread.send(substitute(need(t.message, `turns[${i}].message`), ctx.vars))
        if (!d.accepted) throw new Error(`turn ${i} refused: ${d.reason}`)
      }
      const expect = substitute(t.expectOutput, ctx.vars)
      await thread.settle(pollMs, maxPolls, (th) => th.outputOf(i).some((l) => l.includes(expect)))
      const out = thread.outputOf(i)
      if (!out.some((l) => l.includes(expect))) {
        throw new Error(`turn ${i}: expected ${JSON.stringify(expect)} in ${JSON.stringify(out)} (state ${thread.state})`)
      }
      notes.push(`turn ${i}: ${thread.state}`)
    }
    // A message while a turn runs would be refused; the roster sees the run.
    const roster = await store.refreshRoster()
    if (!roster.some((r) => r.runId === thread.runId && r.bot?.id === bot.id)) {
      throw new Error(`run ${thread.runId} missing from the roster`)
    }
    notes.push(`roster ${roster.length} run(s) in ${store.rosterCalls} agent_list call(s)`)
  } finally {
    await thread.stop().catch(() => {})
  }
  return `${thread.runId}: ${notes.join("; ")}`
}

async function sendFile(ctx: Ctx, step: Step): Promise<string> {
  const space = need(ctx.space, "the Space")
  const store = need(ctx.store, "the store")
  const gen = step.generate as { name: string; bytes: number; seed: string }
  const bytes = xorshiftBytes(gen.bytes, BigInt(gen.seed))
  const sha = sha256Hex(bytes)
  if (typeof step.sha256 === "string" && sha !== step.sha256) {
    throw new Error(`generated ${sha}, the spec says ${step.sha256}`)
  }
  const local = join(ctx.tmp, "send", substitute(gen.name, ctx.vars))
  mkdirSync(dirname(local), { recursive: true })
  writeFileSync(local, bytes)
  const report = await store.attach(local, String(step.subdir ?? "openkoalabot-example-scenario"))
  const path = report.files[0]?.path
  if (!path) throw new Error(`send_file reported no files: ${JSON.stringify(report.files)}`)
  const vars = { ...ctx.vars, path }
  try {
    const guestSha = (await sh(space, substitute(String(step.guestSha256Command), vars))).trim()
    if (guestSha !== sha) throw new Error(`the guest computed ${guestSha}, expected ${sha}`)
  } finally {
    if (typeof step.cleanupCommand === "string") await space.bash(substitute(step.cleanupCommand, vars), 30_000n).catch(() => {})
  }
  return `${report.bytes} bytes to ${path}; sha256 ${sha.slice(0, 12)}… verified by the SDK and the guest`
}

async function teleport(ctx: Ctx, step: Step): Promise<string> {
  const space = need(ctx.space, "the Space")
  const store = need(ctx.store, "the store")
  const app = String(step.app ?? "firefox")
  if (!space.supports(`teleport.${app}`)) throw new Skip(`the Space does not report teleport.${app}`)
  const { receipt, shown } = await store
    .teleport(
      app,
      // Consent is an explicit callback even for a generated profile: approve
      // the manifest's defaults and acknowledge its sensitive items.
      (m) => ({ include: undefined, acknowledgeSensitive: m.items.some((i) => i.isSensitive) }),
      typeof step.scope === "string" ? step.scope : undefined,
    )
    .catch((e: unknown) => {
      // The embedded MIT runtime refuses teleport (HostCapabilityMissing):
      // it ships with Cua Spaces, which the runner does not connect to.
      const reason = teleportNeedsCuaSpaces(e)
      throw reason ? new Skip(reason) : e
    })
  if (shown.length !== 1) throw new Error(`the approver saw ${shown.length} manifests`)
  if (receipt.imported.length === 0) throw new Error(`nothing imported: ${JSON.stringify(receipt.skipped)}`)
  const found = await sh(space, substitute(String(step.verifyCommand), { ...ctx.vars, importRoot: ctx.opts.importRoot }))
  const expect = String(step.expectContains ?? "prefs.js")
  if (!found.includes(expect)) throw new Error(`marker not found in the guest (got ${JSON.stringify(found)})`)
  return `${shown[0].items.length} manifest items, ${receipt.imported.length} imported, ${receipt.bundleBytes} bundle bytes; marker at ${found.trim()}`
}

async function presence(ctx: Ctx, step: Step): Promise<string> {
  const space = requireFeature(ctx, step)
  const info = need(ctx.info, "the Space info")
  const clients = step.clients as Array<{ id: string; displayName: string; agent: boolean }>
  const timeoutMs = num(step, "timeoutMs", 20_000)
  const maxEvents = num(step, "maxEvents", 50)
  const cursor = (step.cursor ?? { x: 0.25, y: 0.75 }) as { x: number; y: number }

  // The second client is a separate runtime with its own temp registry.
  const other = openRuntime({ root: join(ctx.tmp, "client-b"), teleportHome: join(ctx.tmp, "empty-home"), cloudFromEnv: ctx.opts.lane === "cloud" })
  ctx.extra.push(other)
  const target = ctx.opts.lane === "cloud" ? info.id : need(ctx.opts.url, "OPENKOALABOTS_SCENARIO_URL")
  const infoB = await other.spaces().add(target, ctx.opts.token, `${info.name}-b`)
  const spaceB = await other.spaces().space(infoB.id)

  // An agent joins with the SDK's agent identity: it requests its stable
  // presence color, the one its avatar shows.
  const identity = (c: (typeof clients)[number]) =>
    c.agent ? agentIdentity(substitute(c.id, ctx.vars), c.displayName) : { id: substitute(c.id, ctx.vars), displayName: c.displayName, color: "", agent: false }
  // With takeKoalaColor the operator asks for Koala's stable color first, so
  // the server has to assign Koala another one.
  const koalaId = substitute(clients[1].id, ctx.vars)
  const takeKoalaColor = step.takeKoalaColor === true
  const operator = identity(clients[0])
  if (takeKoalaColor) operator.color = presenceColor(koalaId)
  const a = await space.joinPresence(operator, BigInt(timeoutMs))
  const b = await spaceB.joinPresence(identity(clients[1]), BigInt(timeoutMs))
  const seen: string[] = []
  try {
    const meA = await a.me()
    const meB = await b.me()
    // The operator's view: the SDK's roster, folded from every event.
    const roster = step.roster === true ? PresenceRoster.from(meA, await a.roster()) : undefined
    if (!(await b.roster()).some((m) => m.participant.participantId === meA.participantId)) {
      throw new Error("the second client's roster lacks the first")
    }
    const joined = await waitForPresence(a, (e) => e.kind === "joined" && e.participant?.participantId === meB.participantId, timeoutMs, maxEvents, roster)
    if (joined.participant?.displayName !== clients[1].displayName) throw new Error(`joined as ${joined.participant?.displayName}`)
    if (clients[1].agent && joined.participant?.kind !== "agent") throw new Error(`kind ${joined.participant?.kind}`)
    seen.push("joined")
    await b.updateCursor({ displayId: "", windowId: undefined, x: cursor.x, y: cursor.y, visible: true, pressed: false, shape: "arrow", shapeSource: "unspecified", atMs: 0, receivedMs: 0 })
    const moved = await waitForPresence(a, (e) => e.kind === "cursor_moved" && e.participantId === meB.participantId, timeoutMs, maxEvents, roster)
    if (!moved.cursor || Math.abs(moved.cursor.x - cursor.x) > 1e-6 || Math.abs(moved.cursor.y - cursor.y) > 1e-6) {
      throw new Error(`cursor ${JSON.stringify(moved.cursor)}`)
    }
    seen.push("cursor_moved")
    if (roster) {
      const k = roster.get(meB.participantId)
      if (!k || !k.cursor || Math.abs(k.cursor.x - cursor.x) > 1e-6 || Math.abs(k.cursor.y - cursor.y) > 1e-6) {
        throw new Error(`the roster shows ${JSON.stringify(k)}`)
      }
      if (k.agent !== clients[1].agent) throw new Error(`the roster's agent flag is ${k.agent}`)
      if (roster.others.some((o) => o.participantId === meA.participantId)) throw new Error("the roster draws my own cursor")
      // The cursor color and the Bot's avatar color are one color: the app's
      // avatar color, fed from this roster the way the server feeds the page,
      // must be exactly the cursor's.
      const stable = presenceColor(koalaId)
      setAssignedColors(assignedColorsFrom(roster.entries))
      const avatar = botAvatarColor(koalaId)
      const cursorColor = k.color.toLowerCase()
      if (avatar !== cursorColor) throw new Error(`Koala's cursor is ${cursorColor}, its avatar is ${avatar}`)
      if (roster.colorOf(koalaId).toLowerCase() !== cursorColor) throw new Error(`the roster's color for Koala is ${roster.colorOf(koalaId)}`)
      if (takeKoalaColor && avatar === stable) throw new Error(`the operator holds ${stable}, yet Koala kept it`)
      seen.push(`cursor color = avatar color ${avatar}${avatar === stable ? "" : ` (reassigned from ${stable})`}`)
    }
    await b.leave()
    await waitForPresence(a, (e) => e.kind === "left" && e.participantId === meB.participantId, timeoutMs, maxEvents, roster)
    seen.push("left")
    if (roster) {
      if (roster.get(meB.participantId)) throw new Error("Koala is still in the roster after leaving")
      seen.push("roster")
    }
  } finally {
    await b.leave().catch(() => {})
    await a.leave().catch(() => {})
    await other.spaces().remove(infoB.id).catch(() => {})
  }
  return `2 clients; ${seen.join(", ")}`
}

function needModel(ctx: Ctx): void {
  if (!ctx.opts.model) throw new Skip("no model endpoint (OPENKOALABOTS_SCENARIO_MODEL_URL is unset)")
}

/** Settles a thread until its latest turn's output has `expect`. */
async function expectTurn(t: BotThread, expect: string, pollMs: number, maxPolls: number): Promise<void> {
  const has = (th: BotThread) => th.outputOf(th.currentTurn).some((l) => l.includes(expect))
  await t.settle(pollMs, maxPolls, has)
  if (!has(t)) throw new Error(`${t.bot.name}: expected ${JSON.stringify(expect)} in ${JSON.stringify(t.outputOf(t.currentTurn).slice(-8))} (state ${t.state})`)
}

async function routineStep(ctx: Ctx, step: Step): Promise<string> {
  const store = need(ctx.store, "the store")
  needModel(ctx)
  const sub = (v: unknown) => substitute(String(v ?? ""), ctx.vars)
  const b = step.bot as { id: string; name: string }
  const def = step.routine as { title: string; prompt: string; schedule: RoutineSchedule }
  const pollMs = num(step, "pollMs", 1000)
  const maxPolls = num(step, "maxPolls", 600)
  const bot = store.register({ id: sub(b.id), name: b.name, agent: String(step.agent ?? "claude-code") })
  const file = join(ctx.tmp, "routines.json")
  const routines = new RoutineStore(jsonStorage(file), new BotStoreRoutineRunner(store))
  const created = new Date(Date.now() - num(step, "createdAgoMs", 61_000))
  const r = routines.create({ botID: bot.id, title: sub(def.title), prompt: sub(def.prompt), schedule: def.schedule, now: created })
  const notes: string[] = []
  const early = await routines.tick(new Date(created.getTime() + num(step, "notDueAtMs", 30_000)))
  if (early.length) throw new Error(`fired before its slot: ${JSON.stringify(early)}`)
  notes.push("not due before its slot")
  // The real scheduler loop, bounded.
  const interval = num(step, "schedulerIntervalMs", 250)
  routines.startScheduler(interval)
  try {
    for (let i = 0; i < maxPolls && !routines.log.some((l) => l.routineID === r.id); i++) await sleep(interval)
    await sleep(interval * 4)
  } finally {
    routines.stopScheduler()
  }
  const fired = routines.log.filter((l) => l.routineID === r.id)
  if (fired.length !== 1) throw new Error(`the scheduler fired ${fired.length} times: ${JSON.stringify(fired)}`)
  const f = fired[0].firing
  if (f.kind !== "started") throw new Error(`the firing was ${JSON.stringify(f)}`)
  notes.push(`the scheduler fired once (${f.runId})`)
  const t = need(store.thread(f.runId), `the thread for ${f.runId}`)
  try {
    // The clock checks run at the firing's own instant: an everyMinutes slot
    // legitimately comes due again while the first turn is still running.
    const firedAt = new Date(fired[0].at)
    const again = await routines.tick(new Date(firedAt.getTime() + 1_000))
    if (again.length) throw new Error(`a second tick fired ${again.length}`)
    notes.push("no backlog")
    const reloaded = new RoutineStore(jsonStorage(file))
    const saved = reloaded.routine(r.id)
    if (saved?.lastRunID !== f.runId || !saved.lastFiredAt) throw new Error(`the reloaded routine is ${JSON.stringify(saved)}`)
    if (reloaded.due(new Date(firedAt.getTime() + 1_000)).length) throw new Error("the reloaded routine is due again")
    notes.push("reloaded with lastRunID")
    const expectPrompt = sub(step.expectPrompt)
    if (!t.turns[t.currentTurn]?.message.startsWith(expectPrompt)) throw new Error(`the turn's prompt is ${JSON.stringify(t.turns[t.currentTurn]?.message)}`)
    await expectTurn(t, sub(step.expectOutput), pollMs, maxPolls)
    notes.push(`${bot.name} answered (${t.state})`)
  } finally {
    await t.stop().catch(() => {})
  }
  return notes.join("; ")
}

async function groupStep(ctx: Ctx, step: Step): Promise<string> {
  const store = need(ctx.store, "the store")
  needModel(ctx)
  const sub = (v: unknown) => substitute(String(v ?? ""), ctx.vars)
  const pollMs = num(step, "pollMs", 1000)
  const maxPolls = num(step, "maxPolls", 600)
  const bots = (step.bots as Array<{ id: string; name: string }>).map((b) => store.register({ id: sub(b.id), name: b.name, agent: String(step.agent ?? "claude-code") }))
  const groups = new GroupChatStore(new BotStoreGroupMessenger(store))
  const notes: string[] = []
  for (const n of (step.rejectSizes as number[] | undefined) ?? []) {
    const want = n < 2 ? "tooFewBots" : "tooManyBots"
    try {
      groups.create("rejected", Array.from({ length: n }, (_, i) => `bot-${i}`))
      throw new Error(`a group of ${n} was accepted`)
    } catch (e) {
      if (!(e instanceof GroupChatError) || e.code !== want) throw e
    }
  }
  notes.push(`refused sizes ${JSON.stringify(step.rejectSizes ?? [])}`)
  const title = sub(step.title)
  const chat = groups.create(title, bots.map((b) => b.id))
  const threads: BotThread[] = []
  try {
    const ds = await groups.send(sub(step.message), chat.id)
    for (const b of bots) {
      const t = store.threadFor(b.id)
      if (t) threads.push(t)
    }
    const refused = ds.filter((d) => !d.accepted)
    if (refused.length) throw new Error(`not delivered: ${JSON.stringify(refused)}`)
    for (const b of bots) {
      const t = need(store.threadFor(b.id), `${b.name}'s thread`)
      const prompt = t.turns[t.currentTurn]?.message ?? ""
      const others = bots.filter((o) => o.id !== b.id).map((o) => o.name)
      if (!prompt.startsWith(`[group:${title}]`) || !others.every((o) => prompt.includes(o))) throw new Error(`${b.name} was framed as ${JSON.stringify(prompt)}`)
    }
    notes.push(`delivered to ${ds.length}, each framed with the others`)
    const expect = sub(step.expectReply)
    const replied = () => bots.every((b) => chat.messages.some((l) => l.speaker.kind === "bot" && l.speaker.botID === b.id && !l.undelivered && l.text.includes(expect)))
    for (let i = 0; i < maxPolls && !replied(); i++) {
      await groups.collectReplies(chat.id)
      if (!replied()) await sleep(pollMs)
    }
    if (!replied()) throw new Error(`replies: ${JSON.stringify(chat.messages.map((l) => [l.speaker, l.text]))}; states ${threads.map((t) => t.state).join(",")}`)
    const lines = chat.messages.filter((l) => l.speaker.kind === "bot").length
    const extra = await groups.collectReplies(chat.id)
    if (extra.length) throw new Error(`collecting again added ${extra.length}`)
    notes.push(`${lines} attributed replies, none duplicated`)
  } finally {
    for (const t of threads) await t.stop().catch(() => {})
  }
  return `${chat.membershipLabel}; ${notes.join("; ")}`
}

async function deleteSpace(ctx: Ctx): Promise<string> {
  if (!ctx.info) throw new Skip("no Space was opened")
  const said = await ctx.spaces.delete_(ctx.info.id)
  const left = await ctx.spaces.list()
  if (left.some((s) => s.id === ctx.info?.id)) throw new Error(`still registered after delete: ${said}`)
  return said
}

const OPS: Record<string, (ctx: Ctx, step: Step) => Promise<string>> = {
  "space.open": open,
  "stream.desktop": stream,
  "agent.thread": agentThread,
  "file.send": sendFile,
  "teleport.app": teleport,
  "presence.pair": presence,
  "routine.schedule": routineStep,
  "group.chat": groupStep,
  "space.delete": deleteSpace,
}

// ------------------------------------------------------------ driver

export async function runScenario(opts: RunOptions): Promise<ScenarioResult> {
  const started = Date.now()
  const nonce = newNonce()
  const marker = `openkoalabots-${nonce}`
  const tmp = mkdtempSync(join(tmpdir(), "openkoalabot-example-ts-"))
  const log = opts.log ?? (() => {})
  const profile = JSON.parse(readFileSync(resolve(opts.specDir, "fixtures/firefox-profile.json"), "utf8")) as ProfileFixture
  const teleportHome = join(tmp, "teleport-home")
  writeProfile(teleportHome, profile, marker)
  const cua = openRuntime({ root: join(tmp, "client-a"), teleportHome, cloudFromEnv: opts.lane === "cloud" })
  const ctx: Ctx = {
    opts,
    nonce,
    marker,
    vars: { nonce, marker },
    tmp,
    cua,
    spaces: cua.spaces(),
    extra: [],
  }
  const steps: StepResult[] = []
  let failed = false
  try {
    for (const step of opts.spec.steps) {
      const t0 = Date.now()
      if (failed && step.always !== true) {
        steps.push({ id: step.id, status: "skip", ms: 0, detail: "no Space was opened" })
        continue
      }
      try {
        const detail = await OPS[step.op](ctx, step)
        steps.push({ id: step.id, status: "pass", ms: Date.now() - t0, detail })
      } catch (e) {
        const skip = e instanceof Skip
        const detail = e instanceof Error ? e.message : String(e)
        steps.push({ id: step.id, status: skip ? "skip" : "fail", ms: Date.now() - t0, detail })
        // Without a Space nothing after it can run; other steps are
        // independent of each other and still report.
        if (!skip && step.op === "space.open") failed = true
      }
      const last = steps[steps.length - 1]
      log(`${last.status.padEnd(4)} ${step.id} (${last.ms} ms): ${last.detail}`)
    }
  } finally {
    rmSync(tmp, { recursive: true, force: true })
  }
  return {
    impl: "ts",
    lane: opts.lane,
    ok: steps.every((s) => s.status !== "fail"),
    totalMs: Date.now() - started,
    steps,
  }
}
