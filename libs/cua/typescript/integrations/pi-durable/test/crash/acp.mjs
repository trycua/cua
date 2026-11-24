// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Cua AI, Inc.

// CRASH_MODE=acp: the ACP durable driver (crates/cua-agents runner/acp-runner.mjs, the driver
// `durable_driver_spec` builds) against REAL harnesses with a mock model.
//
// Each run is one durable session: a scripted task (shell tool calls that each append a unique line
// to a file in the workspace, plus a write) sent through the real harness binary (claude-agent-acp +
// Claude Code, codex-acp, hermes-acp, opencode) to cua-mock-llm (the harness's model endpoint; no
// model key exists or is needed). The driver runs in a Docker container (its "Space": the runner, the
// harness and its tools share it); the state directory and workspace are on a Docker volume that
// outlives the container, like a job's data directory. This file plays the re-driver by the session
// contract (DESIGN.md): CUA_SESSION*, an epoch per drive that only grows, exit 0 = done, 75 = fenced,
// 78 = permanent, anything else = crashed -> start again at the next epoch (it is NOT the Rust
// coordinator: the coordinator's own faults are redrive.mjs's job; here the system under test is the
// runner and the harness).
//
// Faults (CRASH_ACP_FLAVORS, rotated by seed; `mixed` picks one per drive):
//   runner-kill      SIGKILL of the runner (the container goes with it) while a tool call is in flight
//   harness-kill     SIGKILL of the harness process mid tool call; the runner restarts it in process
//   container-kill   the whole container is killed mid tool call (the runner and the harness with it)
//   coordinator      the re-driver loses track of a live driver: the next epoch starts while the old
//                    container still runs; the old one must stop (exit 75) with nothing more written
//   before-intent    the runner is killed when a permission request arrives, before the intent is journaled
//   after-intent     the runner is killed right after the intent is fsynced, before it answers
//                    (both by a preload, acp-fault.mjs; the shipped runner has no fault hooks)
//   none             a control run without a fault
// Per run it checks (see `check`): the session ends done; every scripted effect appears exactly once,
// or the recovery note listed its call as possibly half-run (recorded, per class); no tool call id gets
// two approved intents; each recovery note matches the ledger at that point; epochs only grow;
// tools.jsonl and events.jsonl agree.
//
// Environment: CRASH_ACP_HARNESSES (default claude-code,openai-codex), CRASH_ACP_BUNDLES (a directory
// with one `<harness>/` per harness, written by `cargo run -p cua-agents --example durable_bundle`; see
// lab/build-acp-image.sh), CRASH_ACP_IMAGE (default cua-acp-driver:dev), CRASH_ACP_FLAVORS,
// CRASH_ACP_HOME=ephemeral|state (where the harness keeps its session files: the container's home, as
// shipped, or under the state directory), CRASH_ACP_STEPS (shell steps per run, default the runs'
// `steps` argument), CRASH_ACP_RUN_TIMEOUT_S, CRASH_ACP_MAX_EPOCHS, CRASH_ACP_EPOCH_PREFIX.
// Needs the `docker` CLI on PATH (DOCKER_HOST as for the other docker modes) and a mock model reachable
// from the container at the URL baked into the bundle.

import { execFile } from "node:child_process"
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, cpSync, appendFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { reportRun } from "./campaign.mjs"
import { here, latencyStats, newSummary, seeded } from "./common.mjs"

const DOCKER = process.env.CRASH_DOCKER ?? "docker"
const HARNESSES = (process.env.CRASH_ACP_HARNESSES ?? "claude-code,openai-codex").split(",").filter(Boolean)
const FLAVORS = (process.env.CRASH_ACP_FLAVORS ?? "runner-kill,harness-kill,container-kill,coordinator,before-intent,after-intent,mixed,none").split(",")
const IMAGE = process.env.CRASH_ACP_IMAGE ?? "cua-acp-driver:dev"
const BUNDLES = process.env.CRASH_ACP_BUNDLES ?? ""
const HOME_MODE = process.env.CRASH_ACP_HOME ?? "ephemeral"
const RUN_TIMEOUT_MS = Number(process.env.CRASH_ACP_RUN_TIMEOUT_S ?? 600) * 1000
const MAX_EPOCHS = Number(process.env.CRASH_ACP_MAX_EPOCHS ?? 8)
const POLL_MS = Number(process.env.CRASH_ACP_POLL_MS ?? 150)
const BACKOFF_MS = Number(process.env.CRASH_ACP_BACKOFF_MS ?? 1000)
const STATE = "/state/session"
const WORK = `${STATE}/work`
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

function docker(args, { input, timeout = 120000 } = {}) {
  return new Promise((resolve) => {
    const child = execFile(DOCKER, args, { maxBuffer: 256 * 1024 * 1024, timeout }, (error, stdout, stderr) =>
      resolve({ code: error ? (typeof error.code === "number" ? error.code : 1) : 0, stdout: String(stdout), stderr: String(stderr) }),
    )
    if (input !== undefined) child.stdin.end(input)
  })
}

const parseLines = (text) =>
  String(text ?? "")
    .split("\n")
    .filter(Boolean)
    .map((l) => {
      try {
        return JSON.parse(l)
      } catch {
        return undefined
      }
    })
    .filter(Boolean)

/**
 * One scripted shell command. Codex (guard mode: the "Ask for approval" mode, a read-only sandbox) asks the
 * client for permission only when a command asks to leave its sandbox, as a real model does after a write fails
 * there, so its commands carry `sandbox_permissions: require_escalated`; the other harnesses just run it.
 */
function shellAction(harness, cmd) {
  if (harness === "openai-codex") return `tool exec_command ${JSON.stringify({ cmd, sandbox_permissions: "require_escalated", justification: "scripted step" })}`
  return `shell ${cmd}`
}

/** The scripted task: shell steps whose effect is one unique line, and a write. */
function makeTask(harness, uid, steps, random) {
  const actions = []
  const effects = []
  for (let k = 1; k <= steps; k++) {
    // Two shapes: the effect first and then a pause (a kill in the pause leaves the effect applied with no
    // outcome), or a pause and then the effect (a kill in the pause leaves it unapplied).
    const post = random() < 0.5
    const line = `s${k} u=${uid}`
    const cmd = post ? `echo "${line}" >> ${WORK}/effects.log && sleep 3` : `sleep 3 && echo "${line}" >> ${WORK}/effects.log`
    actions.push(shellAction(harness, cmd))
    effects.push({ step: k, line, shape: post ? "effect-then-pause" : "pause-then-effect", tool: "shell" })
  }
  // The write: Claude Code's Write tool by name, every other harness through the shell.
  if (harness === "claude-code") {
    actions.push(`tool Write {"file_path":"${WORK}/w-${uid}.txt","content":"write ${uid}"}`)
    effects.push({ step: steps + 1, line: `write ${uid}`, file: `w-${uid}.txt`, tool: "Write" })
  } else {
    actions.push(shellAction(harness, `printf 'write ${uid}' > ${WORK}/w-${uid}.txt`))
    effects.push({ step: steps + 1, line: `write ${uid}`, file: `w-${uid}.txt`, tool: "shell" })
  }
  actions.push(`say finished ${uid}`)
  // The recovery note quotes the first 600 characters of the request; the directive sits behind them so the
  // mock (which reads the LAST `mock:` in the latest user message that has one) keeps following the original
  // script and counts the calls the resumed conversation already holds, like a model that read the note.
  const filler = "Work through the numbered steps of this task in order, one tool call per step, and do not repeat a step that is already done. ".repeat(6)
  const prompt = `Task ${uid}. ${filler}\nmock: ${actions.join("; ")}`
  return { prompt, effects }
}

function faultsFor(flavor, random) {
  const nth = () => 1 + Math.floor(random() * 2)
  const delay = () => 300 + Math.floor(random() * 1500) // inside the 3 s pause of a step
  const mid = (kind) => ({ kind, nth: nth(), delay: delay() })
  switch (flavor) {
    case "runner-kill":
      return [mid("runner")]
    case "harness-kill":
      return [mid("harness")]
    case "container-kill":
      return [mid("container")]
    case "coordinator":
      return [mid("coordinator")]
    case "before-intent":
      return [{ kind: "before_intent", nth: nth() }]
    case "after-intent":
      return [{ kind: "after_intent", nth: nth() }]
    case "mixed": {
      const kinds = ["runner", "harness", "container", "before_intent", "after_intent"]
      return Array.from({ length: 2 + Math.floor(random() * 2) }, () => {
        const kind = kinds[Math.floor(random() * kinds.length)]
        return kind.endsWith("intent") ? { kind, nth: nth() } : mid(kind)
      })
    }
    case "none":
      return []
    default:
      throw new Error(`unknown flavor ${flavor}`)
  }
}

/** One snapshot of the live container's journal (what the supervisor watches). */
async function snapshot(name) {
  const r = await docker(
    [
      "exec",
      name,
      "sh",
      "-c",
      `cd ${STATE} 2>/dev/null || exit 0; cat state.json 2>/dev/null; echo; echo @@; cat tools.jsonl 2>/dev/null; echo @@; grep -E '"type":"(driver_start|session|turn_started|turn_ended|degraded|agent_restart|run_exited)"' events.jsonl 2>/dev/null | cut -c1-4000`,
    ],
    { timeout: 20000 },
  )
  if (r.code !== 0) return null
  const [stateText = "", toolsText = "", eventsText = ""] = r.stdout.split("@@\n")
  let state = {}
  try {
    state = JSON.parse(stateText.trim() || "{}")
  } catch {}
  return { state, tools: parseLines(toolsText), events: parseLines(eventsText) }
}

/** The calls in flight (intent or started, no outcome) in a ledger. */
function inFlight(tools) {
  const last = new Map()
  for (const r of tools) {
    const prev = last.get(r.toolCallId)
    if (r.kind === "outcome") last.set(r.toolCallId, "done")
    else if ((r.kind === "intent" || r.kind === "started") && prev !== "open") last.set(r.toolCallId, "open")
  }
  return [...last].filter(([, s]) => s === "open").map(([id]) => id)
}

async function startDriver({ run, vol, epoch, bundleDir, env, fault, contractName, homeEnv }) {
  const name = `acp-${run}-e${epoch}`
  const args = [
    "create",
    "--init",
    "--name",
    name,
    "--memory",
    process.env.CRASH_ACP_MEMORY ?? "3g",
    "-v",
    `${vol}:/state`,
    "-e",
    `CUA_SESSION=${contractName}`,
    "-e",
    `CUA_SESSION_EPOCH=${epoch}`,
    "-e",
    `CUA_SESSION_ATTEMPT=${epoch}`,
    "-e",
    `CUA_SESSION_RESUME=${epoch > 1 ? 1 : 0}`,
    "-e",
    `CUA_SESSION_STATE_DIR=${STATE}`,
    "-e",
    "CUA_JOB_DATA=/state",
  ]
  for (const [k, v] of Object.entries({ ...env, ...homeEnv })) args.push("-e", `${k}=${v}`)
  // The runner is attacked from outside: a preload (acp-fault.mjs) kills it at the named point.
  if (fault?.kind === "before_intent" || fault?.kind === "after_intent") args.push("-e", `CUA_ACP_TEST_FAULT=${fault.kind}:${fault.nth}`, "-e", "NODE_OPTIONS=--import=/bundle/acp-fault.mjs")
  args.push(IMAGE, "sh", "/bundle/driver.sh")
  let r = await docker(args)
  if (r.code !== 0) throw new Error(`docker create: ${r.stderr}`)
  r = await docker(["cp", `${bundleDir}/.`, `${name}:/bundle`])
  if (r.code !== 0) throw new Error(`docker cp: ${r.stderr}`)
  r = await docker(["start", name])
  if (r.code !== 0) throw new Error(`docker start: ${r.stderr}`)
  return { name, epoch, startedAt: Date.now() }
}

async function inspectExit(name) {
  const r = await docker(["inspect", "-f", "{{.State.Running}} {{.State.ExitCode}}", name], { timeout: 20000 })
  if (r.code !== 0) return { running: false, code: undefined }
  const [running, code] = r.stdout.trim().split(" ")
  return { running: running === "true", code: Number(code) }
}

/** Reads the whole state of a run from its volume (the container may be gone). */
async function readVolume(vol) {
  const r = await docker(
    [
      "run",
      "--rm",
      "-v",
      `${vol}:/state:ro`,
      "--entrypoint",
      "sh",
      IMAGE,
      "-c",
      `cd /state; for f in session/events.jsonl session/tools.jsonl session/state.json session/work/effects.log; do echo "@@FILE $f"; cat $f 2>/dev/null; done; echo "@@FILE ls"; ls session/work 2>/dev/null`,
    ],
    { timeout: 60000 },
  )
  const files = {}
  for (const part of r.stdout.split("@@FILE ").slice(1)) {
    const nl = part.indexOf("\n")
    files[part.slice(0, nl)] = part.slice(nl + 1)
  }
  return {
    events: parseLines(files["session/events.jsonl"]),
    tools: parseLines(files["session/tools.jsonl"]),
    state: (() => {
      try {
        return JSON.parse(files["session/state.json"])
      } catch {
        return {}
      }
    })(),
    effects: (files["session/work/effects.log"] ?? "").split("\n").filter(Boolean),
    ls: (files.ls ?? "").split("\n").filter(Boolean),
  }
}

// ---- the checks ------------------------------------------------------------------------------

/** Applies the runner's own ledger rules to a prefix of tools.jsonl (the entries a recovery note is built from). */
function ledgerEntries(records) {
  const entries = []
  const byId = new Map()
  for (const r of records) {
    let e = byId.get(r.toolCallId)
    if (r.kind === "intent" || r.kind === "started") {
      if (!e || e.outcome) {
        e = { id: r.toolCallId, turn: r.turn }
        entries.push(e)
        byId.set(r.toolCallId, e)
      }
      Object.assign(e, { name: r.name, input: r.input ?? e.input, turn: r.turn })
      if (r.kind === "intent") {
        e.decision = r.decision
        e.unguarded = false
      } else if (!e.decision) e.unguarded = true
    } else if (r.kind === "outcome") {
      if (!e) {
        e = { id: r.toolCallId, name: r.name, unguarded: true, turn: r.turn }
        entries.push(e)
        byId.set(r.toolCallId, e)
      }
      e.outcome = { status: r.status, denied: r.denied }
    }
  }
  return entries
}

function parseNote(text) {
  const out = { completed: [], unknown: [] }
  let section = null
  for (const line of String(text).split("\n")) {
    if (line.startsWith("Tool calls that completed")) section = "completed"
    else if (line.startsWith("Tool calls that may have partially run")) section = "unknown"
    else if (line === "" || !line.startsWith("- ")) {
      if (line === "" || !line.startsWith("- ")) section = line === "" ? null : section
    } else if (section === "completed") out.completed.push(line.slice(2))
    else if (section === "unknown") out.unknown.push({ line: line.slice(2), id: /\(id (.+?)\)/.exec(line)?.[1] })
  }
  return out
}

/** All invariants of one finished run; returns { problems, facts }. */
function check({ vol, data, task, uid, exits, epochsStarted }) {
  const problems = []
  const { events, tools, effects } = data
  const facts = { notes: [], effects: [], intents: 0, unguardedStarts: 0, refusals: 0, duplicatesRefused: 0, degraded: 0, resumed: 0, agentRestarts: 0 }

  // 1. done.
  const last = exits[exits.length - 1]
  const exited = [...events].reverse().find((e) => e.type === "run_exited")
  if (last?.code !== 0 || exited?.code !== 0 || exited?.reason !== "done") problems.push(`the session did not end done (last exit ${last?.code}, run_exited ${JSON.stringify(exited ? { code: exited.code, reason: exited.reason } : null)})`)

  // 2. epochs only grow: the supervisor's, the journal's order, the runners' driver_start lines.
  for (let i = 1; i < epochsStarted.length; i++) if (!(epochsStarted[i] > epochsStarted[i - 1])) problems.push(`epochs started ${epochsStarted.join(",")} do not increase`)
  for (const [file, records] of [["events.jsonl", events], ["tools.jsonl", tools]]) {
    let max = 0
    for (const r of records) {
      if ((r.epoch ?? 0) < max) {
        problems.push(`${file}: a record of epoch ${r.epoch} follows epoch ${max}`)
        break
      }
      max = Math.max(max, r.epoch ?? 0)
    }
  }
  const driverStarts = events.filter((e) => e.type === "driver_start").map((e) => e.epoch)
  for (let i = 1; i < driverStarts.length; i++) if (!(driverStarts[i] > driverStarts[i - 1])) problems.push(`driver_start epochs ${driverStarts.join(",")} do not increase`)

  // 3. one approved intent per call id (a reused id is only legitimate after its outcome).
  const intents = new Map()
  for (const r of tools) {
    if (r.kind === "intent") {
      facts.intents++
      const n = intents.get(r.toolCallId) ?? { open: 0, total: 0 }
      n.total++
      n.open++
      intents.set(r.toolCallId, n)
      if (n.open > 1) problems.push(`tool call ${r.toolCallId} got ${n.open} approved intents with no outcome in between`)
    } else if (r.kind === "outcome") {
      const n = intents.get(r.toolCallId)
      if (n) n.open = 0
    } else if (r.kind === "started" && r.unguarded) facts.unguardedStarts++
    else if (r.kind === "may_have_run") facts.refusals++
    else if (r.kind === "duplicate") facts.duplicatesRefused++
  }

  // 4. each recovery note matches the ledger as it was when the note was built.
  const turnStarts = events.filter((e) => e.type === "turn_started" && (e.recovery || e.noteAttached))
  for (const ev of turnStarts) {
    const prefix = tools.filter((t) => t.ts <= ev.ts)
    const es = ledgerEntries(prefix)
    const completed = es.filter((e) => e.outcome && !e.outcome.denied)
    const unknown = es.filter((e) => !e.outcome && (e.decision === "approve" || e.unguarded))
    const note = parseNote(ev.prompt)
    const wantUnknown = new Set(unknown.slice(-20).map((e) => e.id))
    const gotUnknown = new Set(note.unknown.map((u) => u.id))
    const sameUnknown = wantUnknown.size === gotUnknown.size && [...wantUnknown].every((id) => gotUnknown.has(id))
    const sameCompleted = note.completed.length === Math.min(completed.length, 40)
    facts.notes.push({ epoch: ev.epoch, completed: completed.length, unknown: unknown.length, listedCompleted: note.completed.length, listedUnknown: note.unknown.length, match: sameUnknown && sameCompleted })
    if (!sameUnknown) problems.push(`recovery note of epoch ${ev.epoch}: lists may-have-run ${[...gotUnknown].join(",") || "none"}, the ledger has ${[...wantUnknown].join(",") || "none"}`)
    if (!sameCompleted) problems.push(`recovery note of epoch ${ev.epoch}: lists ${note.completed.length} completed calls, the ledger has ${completed.length}`)
    facts.noteUnknownIds = [...(facts.noteUnknownIds ?? []), ...gotUnknown]
    facts.noteUnknownLines = [...(facts.noteUnknownLines ?? []), ...note.unknown.map((u) => u.line)]
  }

  // 5. the effects.
  const entries = ledgerEntries(tools)
  const noteLines = facts.noteUnknownLines ?? []
  for (const eff of task.effects) {
    const count = eff.file ? (data.ls.includes(eff.file) ? 1 : 0) : effects.filter((l) => l === eff.line).length
    const mine = entries.filter((e) => `${e.name ?? ""} ${e.input ?? ""}`.includes(eff.line) || (eff.file && `${e.name ?? ""} ${e.input ?? ""}`.includes(eff.file)))
    const guarded = mine.some((e) => e.decision === "approve")
    const flagged = mine.some((e) => facts.noteUnknownIds?.includes(e.id)) || noteLines.some((l) => l.includes(eff.line) || (eff.file && l.includes(eff.file)))
    let verdict
    if (count === 1) verdict = flagged ? "once+flagged" : "once"
    else if (count === 0) verdict = flagged ? "lost+flagged" : mine.length === 0 ? "not-reached" : "lost-unflagged"
    else if (!guarded) verdict = "duplicate-unguarded"
    // The ledger shows the step completed and the model issued it again under a new request id (the scripted mock
    // does not read the recovery note; Claude Code also drops tool_use/tool_result pairs its transcript had not
    // flushed when the container was killed, so the resumed conversation has no trace of a step that finished).
    else if (mine.some((e) => e.outcome && !e.outcome.denied) && mine.length > 1) verdict = "duplicate-reissued-after-complete"
    // A guarded effect that ran twice is a bug unless the first call was listed in a recovery note as one that may
    // have run and the model then chose to issue the step again under a new request id (the runner refuses a repeat
    // of the same id; whether to repeat the step is the model's call, which a scripted mock cannot make).
    else verdict = flagged ? "duplicate-after-note" : "duplicate-guarded"
    facts.effects.push({ step: eff.step, tool: eff.tool, shape: eff.shape, count, guarded, flagged, calls: mine.length, verdict })
    // "not-reached": the harness's request for the call never got an answer and the runner journaled nothing (killed
    // before the intent): the effect did not happen and no ledger record exists to list. Safe (nothing applied twice);
    // counted, with the scripted mock skipping the step (a real model sees the harness's interrupted-call marker).
    if (verdict === "lost-unflagged") problems.push(`effect ${eff.step} (${eff.tool}) is missing and no recovery note listed its call`)
    if (verdict === "duplicate-guarded") problems.push(`effect ${eff.step} (${eff.tool}) ran ${count} times although the call was guarded`)
  }
  // Any line of effects.log that no step wrote (a stray or foreign effect).
  for (const l of effects) if (!task.effects.some((e) => e.line === l)) problems.push(`unexpected line in effects.log: ${l}`)

  // 6. ledger and journal agree.
  const eventIds = new Set()
  const completedIds = new Set()
  for (const e of events) {
    if (e.type === "update" && (e.update?.sessionUpdate === "tool_call" || e.update?.sessionUpdate === "tool_call_update")) {
      eventIds.add(e.update.toolCallId)
      if (e.update.status === "completed" || e.update.status === "failed") completedIds.add(e.update.toolCallId)
    }
    if (e.type === "permission" && e.toolCall?.toolCallId) eventIds.add(e.toolCall.toolCallId)
  }
  for (const r of tools) {
    if (!eventIds.has(r.toolCallId)) {
      problems.push(`ledger record ${r.kind} for ${r.toolCallId} has no counterpart in events.jsonl`)
      break
    }
  }
  for (const r of tools) {
    if (r.kind === "outcome" && !r.denied && !completedIds.has(r.toolCallId)) {
      problems.push(`outcome of ${r.toolCallId} in tools.jsonl has no completed/failed update in events.jsonl`)
      break
    }
  }
  for (const id of completedIds) {
    if (!tools.some((r) => r.toolCallId === id && r.kind === "outcome")) {
      problems.push(`events.jsonl has a finished update for ${id} that tools.jsonl lacks`)
      break
    }
  }

  // Facts about the harness.
  const init = events.find((e) => e.type === "initialized")
  facts.capabilities = init ? { loadSession: init.agentCapabilities?.loadSession ?? false, resume: Boolean(init.agentCapabilities?.sessionCapabilities?.resume), agent: init.agentInfo?.name } : null
  facts.resumed = events.filter((e) => e.type === "session" && e.resumed).length
  facts.degraded = events.filter((e) => e.type === "degraded").length
  facts.agentRestarts = events.filter((e) => e.type === "agent_restart").length
  facts.permissionNames = [...new Set(tools.filter((r) => r.kind === "intent").map((r) => r.name))]
  facts.unguardedNames = [...new Set(tools.filter((r) => r.kind === "started" && r.unguarded).map((r) => r.name))]
  facts.notices = events.filter((e) => e.type === "notice").map((e) => e.message).slice(0, 5)
  return { problems, facts }
}

// ---- one run -----------------------------------------------------------------------------------

async function oneRun({ harness, run, seed, steps, summary, out }) {
  const random = seeded(seed)
  const flavor = FLAVORS[seed % FLAVORS.length]
  const uid = `${seed.toString(36)}${Date.now().toString(36).slice(-4)}`
  const task = makeTask(harness, uid, steps, random)
  const faults = faultsFor(flavor, random)
  const record = { run, seed, mode: "acp", harness, flavor, steps, faults, recoveries: [], log: [], startedAt: Date.now(), kills: 0, epochs: 0 }
  const vol = `acp-${harness}-${seed}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`
  const bundleSrc = join(BUNDLES, harness)
  const bundleDir = mkdtempSync(join(tmpdir(), `acp-bundle-${harness}-`))
  const stat = (summary.byHarness[harness] ??= { runs: 0, ok: 0, kills: {}, epochs: 0, resumes: 0, resumedSessions: 0, degraded: 0, agentRestarts: 0, mayHaveRunRefusals: 0, duplicatesRefused: 0, effects: {}, resumeMs: [], notes: 0, notesMatching: 0, permissionNames: {}, unguardedNames: {}, flavors: {}, capabilities: null })
  let ok = true
  const problems = []
  const exits = []
  const epochsStarted = []
  const killLog = []
  const latencies = []
  try {
    cpSync(bundleSrc, bundleDir, { recursive: true })
    cpSync(join(here, "acp-fault.mjs"), join(bundleDir, "acp-fault.mjs"))
    const runJson = JSON.parse(readFileSync(join(bundleDir, "run.json"), "utf8"))
    runJson.prompt = task.prompt
    const homeEnv = {}
    if (HOME_MODE === "state") {
      // The harness keeps its session files under the state directory, so a new container can load them.
      const home = "/state/agent-home"
      runJson.env = {
        ...runJson.env,
        CLAUDE_CONFIG_DIR: `${home}/claude`,
        CODEX_HOME: `${home}/codex`,
        HERMES_HOME: `${home}/hermes`,
        XDG_DATA_HOME: `${home}/xdg-data`,
        XDG_STATE_HOME: `${home}/xdg-state`,
      }
    }
    writeFileSync(join(bundleDir, "run.json"), JSON.stringify(runJson, null, 2), { mode: 0o600 })
    const spec = JSON.parse(readFileSync(join(bundleSrc, "..", `${harness}.spec.json`), "utf8"))
    await docker(["volume", "create", vol])
    // Files the launch wants in a state directory path (codex's config.toml) are copied there first.
    const staged = { "openai-codex": ["codex-home", "config.toml", "codex"], hermes: ["hermes-home", "config.yaml", "hermes"] }[harness]
    if (HOME_MODE === "state" && staged) {
      const [dir, file, home] = staged
      const cfg = readFileSync(join(bundleSrc, dir, file), "utf8")
      const r = await docker(["run", "--rm", "-i", "-v", `${vol}:/state`, "--entrypoint", "sh", IMAGE, "-c", `mkdir -p /state/agent-home/${home} && cat > /state/agent-home/${home}/${file}`], { input: cfg })
      if (r.code !== 0) throw new Error(`could not stage the ${harness} config: ${r.stderr}`)
    }
    const deadline = Date.now() + RUN_TIMEOUT_MS
    let epoch = 0
    let pendingKillAt
    while (epoch < MAX_EPOCHS) {
      epoch++
      epochsStarted.push(epoch)
      const fault = faults[epoch - 1]
      const drive = await startDriver({ run: `${seed}-${harness}`, vol, epoch, bundleDir, env: spec.env, fault, contractName: `crash-${seed}`, homeEnv })
      const startedAt = drive.startedAt
      if (pendingKillAt) {
        latencies.push({ kind: pendingKillAt.kind, killToStartMs: startedAt - pendingKillAt.at })
        pendingKillAt.latency = latencies[latencies.length - 1]
      }
      const seenInFlight = new Set()
      let scheduled
      let sawDriverStart
      let sawSession
      let sawTurn
      let exit
      let coordinatorSpawned
      let oldDrive
      for (;;) {
        if (Date.now() > deadline) throw new Error(`run not finished after ${RUN_TIMEOUT_MS / 1000}s (epoch ${epoch})`)
        const info = await inspectExit(drive.name)
        if (!info.running) {
          exit = info.code
          break
        }
        const snap = await snapshot(drive.name)
        if (snap) {
          const now = Date.now()
          if (!sawDriverStart && snap.events.some((e) => e.type === "driver_start" && e.epoch === epoch)) sawDriverStart = now
          if (!sawSession && snap.events.some((e) => e.type === "session" && e.epoch === epoch)) sawSession = now
          if (!sawTurn && snap.events.some((e) => e.type === "turn_started" && e.epoch === epoch)) sawTurn = now
          // The first new tool call of a resumed drive: how long after the kill until work goes on.
          const firstIntent = snap.tools.find((t) => t.epoch === epoch && (t.kind === "intent" || t.kind === "started"))
          if (pendingKillAt && !pendingKillAt.firstCall && firstIntent) {
            pendingKillAt.firstCall = now - pendingKillAt.at
            if (pendingKillAt.latency) pendingKillAt.latency.firstCall = pendingKillAt.firstCall
          }
          if (pendingKillAt && sawSession && !pendingKillAt.sessionMs) {
            pendingKillAt.sessionMs = sawSession - pendingKillAt.at
            if (pendingKillAt.latency) pendingKillAt.latency.sessionMs = pendingKillAt.sessionMs
          }
          if (pendingKillAt && sawTurn && !pendingKillAt.turnMs) {
            pendingKillAt.turnMs = sawTurn - pendingKillAt.at
            if (pendingKillAt.latency) pendingKillAt.latency.turnMs = pendingKillAt.turnMs
          }
          if (fault && ["runner", "harness", "container", "coordinator"].includes(fault.kind) && !scheduled) {
            for (const id of inFlight(snap.tools)) seenInFlight.add(id)
            // Hermes reports a tool call only once it has finished (started and outcome arrive together), so
            // its calls are never visible in flight: attack the window after the previous outcome of this drive.
            const lateReporter = harness === "hermes"
            const doneHere = snap.tools.filter((t) => t.epoch === epoch && t.kind === "outcome").length
            if (seenInFlight.size >= fault.nth || (lateReporter && sawTurn && doneHere >= fault.nth - 1)) {
              scheduled = true
              if (process.env.CRASH_ACP_DEBUG) console.error(`[debug] run ${seed} epoch ${epoch}: ${fault.kind} fault scheduled in ${fault.delay}ms (sawTurn ${Boolean(sawTurn)}, ${seenInFlight.size} in flight)`)
              const pid = snap.state.runnerPid
              const agentPid = snap.state.agentPid
              setTimeout(async () => {
                const at = Date.now()
                let r
                if (fault.kind === "runner") r = await docker(["exec", drive.name, "kill", "-9", String(pid)], { timeout: 20000 })
                else if (fault.kind === "harness") r = await docker(["exec", drive.name, "kill", "-9", String(agentPid)], { timeout: 20000 })
                else if (fault.kind === "container") r = await docker(["kill", drive.name], { timeout: 20000 })
                if (fault.kind === "coordinator") {
                  coordinatorSpawned = true
                  killLog.push({ kind: "coordinator", at, epoch })
                  return
                }
                if (process.env.CRASH_ACP_DEBUG) console.error(`[debug] run ${seed}: ${fault.kind} fault fired, exit ${r?.code} ${String(r?.stderr ?? "").slice(0, 100)}`)
                killLog.push({ kind: fault.kind, at, epoch, ok: r?.code === 0 })
                if (fault.kind !== "harness") pendingKillAt = { kind: fault.kind, at }
                else pendingKillAt = { kind: "harness", at }
              }, fault.delay)
            }
          }
        }
        if (coordinatorSpawned) {
          // The re-driver lost the live driver: the next epoch starts now, the old container keeps running.
          oldDrive = drive
          break
        }
        await sleep(POLL_MS)
      }
      if (oldDrive) {
        const kill = killLog.find((k) => k.kind === "coordinator" && k.epoch === epoch)
        pendingKillAt = { kind: "coordinator", at: kill.at }
        record.kills++
        stat.kills.coordinator = (stat.kills.coordinator ?? 0) + 1
        epoch++
        epochsStarted.push(epoch)
        const next = await startDriver({ run: `${seed}-${harness}`, vol, epoch, bundleDir, env: spec.env, fault: undefined, contractName: `crash-${seed}`, homeEnv })
        latencies.push({ kind: "coordinator", killToStartMs: next.startedAt - kill.at })
        pendingKillAt.latency = latencies[latencies.length - 1]
        // The old driver must stop on its own (fenced, 75) within a few seconds.
        const oldDeadline = Date.now() + 30000
        let oldInfo
        for (;;) {
          oldInfo = await inspectExit(oldDrive.name)
          if (!oldInfo.running || Date.now() > oldDeadline) break
          await sleep(250)
        }
        const staleExit = oldInfo.running ? "still-running" : oldInfo.code
        record.staleDriverExit = staleExit
        stat.staleDriverExits = { ...(stat.staleDriverExits ?? {}), [staleExit]: (stat.staleDriverExits?.[staleExit] ?? 0) + 1 }
        if (staleExit !== 75) problems.push(`the old driver (epoch ${oldDrive.epoch}) did not stop fenced: ${staleExit}`)
        await saveLogs(oldDrive.name, record)
        await docker(["rm", "-f", oldDrive.name])
        exits.push({ epoch: oldDrive.epoch, code: staleExit })
        // Follow the new driver.
        const result = await followDriver(next, deadline, pendingKillAt)
        exits.push({ epoch, code: result.exit })
        await saveLogs(next.name, record)
        await docker(["rm", "-f", next.name])
        if (result.exit === 0) break
        if (result.exit === 78) {
          problems.push("the driver ended permanent (78)")
          break
        }
        await sleep(BACKOFF_MS)
        continue
      }
      exits.push({ epoch, code: exit })
      await saveLogs(drive.name, record)
      await docker(["rm", "-f", drive.name])
      const killedHere = killLog.filter((k) => k.epoch === epoch && k.kind !== "coordinator")
      if (exit === 0) break
      if (exit === 78) {
        problems.push("the driver ended permanent (78)")
        break
      }
      if (exit === 75) {
        problems.push(`epoch ${epoch} was fenced (75) with no newer epoch started`)
        break
      }
      // Crashed (SIGKILL: 137, or the like): count a kill when we or the runner's own fault caused it.
      if (fault && !(fault.kind === "harness") && !killedHere.length && fault.kind !== "before_intent" && fault.kind !== "after_intent") problems.push(`epoch ${epoch} crashed (exit ${exit}) without our fault landing`)
      if (fault?.kind === "before_intent" || fault?.kind === "after_intent") {
        killLog.push({ kind: fault.kind, at: Date.now() - 1, epoch, ok: true, selfKilled: true })
        pendingKillAt = { kind: fault.kind, at: Date.now() }
      }
      if (fault && ["runner", "container", "before_intent", "after_intent"].includes(fault.kind)) {
        record.kills++
        stat.kills[fault.kind] = (stat.kills[fault.kind] ?? 0) + 1
      }
      if (!fault && exit !== 0) problems.push(`epoch ${epoch} crashed (exit ${exit}) with no fault planned`)
      await sleep(BACKOFF_MS)
    }
    record.epochs = epoch
    // A harness kill does not end the container: count it from the journal.
    const data = await readVolume(vol)
    const harnessKills = killLog.filter((k) => k.kind === "harness")
    record.kills += harnessKills.length
    stat.kills.harness = (stat.kills.harness ?? 0) + harnessKills.length
    const { problems: found, facts } = check({ vol, data, task, uid, exits, epochsStarted })
    problems.push(...found)
    record.facts = facts
    record.exits = exits
    record.killLog = killLog
    // Fault accounting: a planned kill that never landed proves nothing.
    // (A fault for an epoch that never started is not a planned fault: a harness kill does not start a new epoch.)
    for (const f of faults.slice(0, Math.max(1, record.epochs))) {
      const landed =
        f.kind === "harness" ? killLog.some((k) => k.kind === "harness" && k.ok) : f.kind === "before_intent" || f.kind === "after_intent" ? killLog.some((k) => k.kind === f.kind) : killLog.some((k) => k.kind === f.kind) || (f.kind === "coordinator" && killLog.some((k) => k.kind === "coordinator"))
      // A `mixed` plan draws several faults for successive drives; one aimed at a call the final drive never reached
      // is counted as skipped, not failed. A single-fault flavor must land.
      if (!landed && flavor === "mixed") record.faultsSkipped = (record.faultsSkipped ?? 0) + 1
      else if (!landed) problems.push(`planned fault ${f.kind} (call ${f.nth}) never landed: the session finished first`)
    }
    // Aggregates.
    stat.runs++
    stat.epochs += record.epochs
    stat.resumes += Math.max(0, record.epochs - 1) + facts.agentRestarts
    stat.resumedSessions += facts.resumed
    stat.degraded += facts.degraded
    stat.agentRestarts += facts.agentRestarts
    stat.mayHaveRunRefusals += facts.refusals
    stat.duplicatesRefused += facts.duplicatesRefused
    stat.notes += facts.notes.length
    stat.notesMatching += facts.notes.filter((n) => n.match).length
    stat.flavors[flavor] = (stat.flavors[flavor] ?? 0) + 1
    stat.capabilities ??= facts.capabilities
    for (const n of facts.permissionNames) stat.permissionNames[n] = (stat.permissionNames[n] ?? 0) + 1
    for (const n of facts.unguardedNames) stat.unguardedNames[n] = (stat.unguardedNames[n] ?? 0) + 1
    for (const e of facts.effects) {
      const key = `${e.tool}:${e.verdict}`
      stat.effects[key] = (stat.effects[key] ?? 0) + 1
    }
    for (const l of pendingLatencies(killLog, latencies)) stat.resumeMs.push(l)
    record.latencies = latencies
    record.recoveries = []
  } catch (error) {
    ok = false
    record.error = String(error?.stack ?? error)
  } finally {
    try {
      const data = await readVolume(vol)
      record.tail = { events: data.events.slice(-12).map((e) => JSON.stringify(e).slice(0, 300)), tools: data.tools.slice(-12), effects: data.effects }
    } catch {}
    await docker(["volume", "rm", "-f", vol])
    rmSync(bundleDir, { recursive: true, force: true })
  }
  if (problems.length > 0) {
    ok = false
    record.error = `${record.error ? `${record.error}; ` : ""}${problems.join("; ")}`
  }
  record.ok = ok
  if (ok) stat.ok++
  summary.kills += record.kills
  for (const k of killLog) summary.faults[k.kind] = (summary.faults[k.kind] ?? 0) + 1
  if (ok) delete record.tail
  reportRun(summary, record, out)
}

function pendingLatencies(killLog, latencies) {
  return latencies.filter((l) => l.firstCall !== undefined).map((l) => l.firstCall)
}

async function saveLogs(name, record) {
  const r = await docker(["exec", name, "sh", "-c", "tail -c 3000 /bundle/agent.log 2>/dev/null; echo ---; tail -c 1500 /bundle/install.log 2>/dev/null"], { timeout: 15000 })
  const text = r.code === 0 ? r.stdout : (await docker(["cp", `${name}:/bundle/agent.log`, "-"], { timeout: 15000 })).stdout.slice(-3000)
  record.log.push(`[${name}] ${text.slice(-3000)}`)
  if (record.log.length > 6) record.log.shift()
}

async function followDriver(drive, deadline, pendingKillAt) {
  for (;;) {
    if (Date.now() > deadline) throw new Error("run not finished in time")
    const info = await inspectExit(drive.name)
    if (!info.running) return { exit: info.code }
    const snap = await snapshot(drive.name)
    if (snap && pendingKillAt && !pendingKillAt.firstCall) {
      const firstIntent = snap.tools.find((t) => t.epoch === drive.epoch && (t.kind === "intent" || t.kind === "started"))
      if (firstIntent) {
        pendingKillAt.firstCall = Date.now() - pendingKillAt.at
        if (pendingKillAt.latency) pendingKillAt.latency.firstCall = pendingKillAt.firstCall
      }
    }
    await sleep(POLL_MS)
  }
}

export async function campaign({ runs, steps, out, seedBase }) {
  if (!BUNDLES) throw new Error("CRASH_MODE=acp needs CRASH_ACP_BUNDLES (see test/crash/acp.mjs and lab/build-acp-image.sh)")
  const summary = newSummary({ mode: "acp", image: IMAGE, homeMode: HOME_MODE, harnesses: HARNESSES, byHarness: {} })
  const stepCount = Number(process.env.CRASH_ACP_STEPS ?? steps)
  let run = 0
  for (const harness of HARNESSES) {
    for (let i = 0; i < runs; i++, run++) {
      await oneRun({ harness, run, seed: seedBase + i, steps: stepCount, summary, out })
    }
  }
  for (const s of Object.values(summary.byHarness)) {
    s.resumeLatencyMs = latencyStats(s.resumeMs)
    delete s.resumeMs
  }
  return summary
}
