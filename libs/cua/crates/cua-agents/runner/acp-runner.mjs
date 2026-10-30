// cua agent runner: drives one ACP agent inside the sandbox and keeps every
// fact about the run in files, so the run outlives any client.
//
//   node acp-runner.mjs <run_dir>
//
// Reads   <run_dir>/run.json      what to run (written by the SDK)
//         <run_dir>/secrets.json  env for the agent only (0600, optional)
//         <run_dir>/inbox/*.json  {"op":"prompt","text":..} | {"op":"cancel"} | {"op":"stop"}
// Writes  <run_dir>/events.jsonl  one JSON event per line, seq-numbered
//         <run_dir>/state.json    {"status", "turn", "sessionId", ...}
//         <run_dir>/agent.log     the agent's stderr
//
// The protocol is the Agent Client Protocol, spoken with the official
// TypeScript SDK (@agentclientprotocol/sdk). Permission requests are
// answered with the most permissive "allow" option: the sandbox is the
// boundary. The runner advertises no fs or terminal capability, so agents use
// their own tools. Nothing here parses harness-specific output.
//
// DURABLE MODE (run.json `durable`; the session env contract overrides it):
//
//   durable: { epoch, resume, stateDir, session, attempt, space,
//              maxAgentRestarts = 3, policy = "ask-model" | "rerun-safe",
//              guard = true, ship = null | {ackFile, ackTimeoutMs} }
//   env:     CUA_SESSION, CUA_SESSION_EPOCH, CUA_SESSION_ATTEMPT,
//            CUA_SESSION_RESUME (0|1), CUA_SESSION_SPACE (the durable Space's
//            name), CUA_SESSION_STATE_DIR (empty: $CUA_JOB_DATA/session),
//            CUA_SESSION_HEARTBEAT_SECS (print a CUA-HEARTBEAT line this often);
//            with a Space also CUA_SESSION_SPACE_ADDR/_TOKEN/_LEASE
//
// - The journal (events.jsonl, state.json, tools.jsonl) lives in stateDir and
//   is written through an epoch fence: `<stateDir>/epoch` holds the floor,
//   raised atomically at start (compare-and-raise under an O_EXCL lock). Every
//   write re-reads it; a floor above this runner's epoch stops the runner with
//   exit code 75 and nothing more is written.
// - tools.jsonl is the tool ledger. A permission request journals an `intent`
//   (fsync) BEFORE it is answered, and a tool call id is approved once. A
//   repeat request for an id whose outcome is unknown is NOT approved: it is
//   recorded as `may_have_run` and refused (policy "rerun-safe" re-approves
//   read-only kinds). `outcome` records come from ACP tool_call updates;
//   tools that never ask permission only get `started`/`outcome` records,
//   flagged `unguarded` (exactly-once for them needs a harness shim).
// - A restarted runner (or an in-process agent restart after a crash, at most
//   maxAgentRestarts) resumes the ACP session (session/resume or
//   session/load; a harness with neither gets a new session and the run is
//   flagged `degraded`) and sends a RECOVERY NOTE as the first prompt of the
//   resumed turn. A turn that already has `turn_ended` is never replayed.
// - With `ship`, the host mirrors the journal to the Cua Volume
//   (cua_volume::journal) and acknowledges it in <stateDir>/<ackFile>
//   ({epoch, end: {"tools.jsonl": bytes}}). An intent is answered only once
//   the ack covers it, so a re-drive on any host sees it; no ack within
//   ackTimeoutMs refuses the call (journaled as a denied outcome).
// - Exit codes: 0 finished, 78 permanent (authentication, bad config, agent
//   not found), 75 fenced, anything else crashed.
//
// One thing is provider-specific: run.json may carry a `keyCheck` (see
// checkKey below) so a rejected API key ends the first turn in seconds.

import * as acp from "@agentclientprotocol/sdk";
import { spawn } from "node:child_process";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { Readable, Writable } from "node:stream";

export const EXIT_PERMANENT = 78;
export const EXIT_FENCED = 75;

const dir = process.argv[2];
if (!dir) {
  console.error("usage: acp-runner.mjs <run_dir>");
  process.exit(2);
}
const P = (f) => path.join(dir, f);
const cfg = JSON.parse(fs.readFileSync(P("run.json"), "utf8"));
let secrets = {};
try {
  secrets = JSON.parse(fs.readFileSync(P("secrets.json"), "utf8"));
} catch {}

// ---- durable configuration ---------------------------------------------------

// ${NAME} and a leading ~/ in launch paths: a durable bundle is built before
// the machine, its home and its run directory are known.
const expandVars = (s) =>
  typeof s !== "string"
    ? s
    : s
        .replace(/\$\{([A-Za-z_][A-Za-z0-9_]*)\}/g, (m, k) => process.env[k] ?? cfg.env?.[k] ?? m)
        .replace(/^~(?=\/|$)/, os.homedir());

function loadDurable() {
  if (!cfg.durable) return null;
  const d = cfg.durable;
  const e = process.env;
  const epoch = Number(e.CUA_SESSION_EPOCH ?? d.epoch);
  const out = {
    epoch,
    resume: e.CUA_SESSION_RESUME !== undefined ? e.CUA_SESSION_RESUME === "1" : Boolean(d.resume),
    // The coordinator may leave the state dir empty: then the job's data dir.
    stateDir: expandVars(e.CUA_SESSION_STATE_DIR || d.stateDir || (e.CUA_JOB_DATA ? path.join(e.CUA_JOB_DATA, "session") : "")),
    session: e.CUA_SESSION || d.session || "",
    attempt: Number(e.CUA_SESSION_ATTEMPT ?? d.attempt ?? 1),
    space: e.CUA_SESSION_SPACE ?? d.space ?? "",
    maxAgentRestarts: Number.isInteger(d.maxAgentRestarts) ? d.maxAgentRestarts : 3,
    policy: d.policy === "rerun-safe" ? "rerun-safe" : "ask-model",
    // Keep the harness asking permission (so every tool call is guarded):
    // no switch to its full-access mode, and "allow once", never "always".
    guard: d.guard !== false,
    // Journal shipping to the Cua Volume: the host that drives the run
    // mirrors the journal and writes `ackFile` ({epoch, end: {file: bytes}})
    // once bytes are on the Volume. An intent is answered only after its ack.
    ship:
      d.ship && typeof d.ship === "object"
        ? { ackFile: String(d.ship.ackFile || "ship.ack"), ackTimeoutMs: Number(d.ship.ackTimeoutMs ?? 30_000) }
        : null,
  };
  if (!Number.isSafeInteger(epoch) || epoch < 1) {
    console.error(`durable: CUA_SESSION_EPOCH must be a positive integer (got ${e.CUA_SESSION_EPOCH ?? d.epoch})`);
    process.exit(EXIT_PERMANENT);
  }
  if (!out.stateDir) {
    // A journal in a directory that dies with the driver is no journal.
    console.error("durable: no state directory (CUA_SESSION_STATE_DIR, durable.stateDir or CUA_JOB_DATA)");
    process.exit(EXIT_PERMANENT);
  }
  return out;
}
const durable = loadDurable();
// Where the journal lives: the run directory, or the durable state directory.
const J = (f) => path.join(durable?.stateDir ?? dir, f);
if (durable) fs.mkdirSync(durable.stateDir, { recursive: true, mode: 0o700 });

if (durable) {
  process.env.CUA_RUN_DIR ??= path.resolve(dir);
  // The expanded directory, never the `~/...` the session was given: `${CUA_SESSION_STATE_DIR}/work`
  // is the harness's cwd, and a harness refuses a cwd that is not absolute.
  process.env.CUA_SESSION_STATE_DIR = durable.stateDir;
  cfg.cwd = expandVars(cfg.cwd);
  cfg.agent = { ...cfg.agent, command: expandVars(cfg.agent.command), args: (cfg.agent.args ?? []).map(expandVars) };
  cfg.env = Object.fromEntries(Object.entries(cfg.env ?? {}).map(([k, v]) => [k, expandVars(v)]));
  try {
    fs.mkdirSync(cfg.cwd, { recursive: true });
  } catch {}
}

const secretValues = [
  ...Object.values(secrets),
  ...(cfg.secretEnv ?? []).map((n) => process.env[n]),
].filter((v) => typeof v === "string" && v.length >= 8);

// ---- the epoch fence -----------------------------------------------------------

const sleepSync = (ms) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms);
let child = null;

function readFloor() {
  try {
    return Number(fs.readFileSync(J("epoch"), "utf8").trim()) || 0;
  } catch {
    return 0;
  }
}

// A newer epoch exists: stop at once, write nothing.
function fenced(floor) {
  try {
    child?.kill("SIGKILL");
  } catch {}
  process.stderr.write(`fenced: epoch ${durable.epoch} is below the floor ${floor}; stopping\n`);
  process.exit(EXIT_FENCED);
}

function checkFence() {
  if (!durable) return;
  const floor = readFloor();
  if (floor > durable.epoch) fenced(floor);
}

function fsyncDir(d) {
  try {
    const fd = fs.openSync(d, "r");
    try {
      fs.fsyncSync(fd);
    } finally {
      fs.closeSync(fd);
    }
  } catch {}
}

// Compare-and-raise the floor to our epoch under an O_EXCL lock.
function raiseFloor() {
  const lock = J("epoch.lock");
  const deadline = Date.now() + 10_000;
  for (;;) {
    try {
      const fd = fs.openSync(lock, "wx");
      fs.writeSync(fd, JSON.stringify({ pid: process.pid, ts: Date.now() }));
      fs.closeSync(fd);
      break;
    } catch (e) {
      if (e.code !== "EEXIST") throw e;
      try {
        // A lock left by a killed process.
        if (Date.now() - fs.statSync(lock).mtimeMs > 5000) fs.unlinkSync(lock);
      } catch {}
      if (Date.now() > deadline) {
        console.error("durable: the epoch lock stayed busy for 10s");
        process.exit(1);
      }
      sleepSync(15);
    }
  }
  let above = 0;
  try {
    const floor = readFloor();
    if (floor > durable.epoch) above = floor;
    else if (floor < durable.epoch) {
      const tmp = J(`.epoch.${process.pid}.tmp`);
      const fd = fs.openSync(tmp, "w", 0o600);
      fs.writeSync(fd, String(durable.epoch));
      fs.fsyncSync(fd);
      fs.closeSync(fd);
      fs.renameSync(tmp, J("epoch"));
      fsyncDir(durable.stateDir);
    }
  } finally {
    try {
      fs.unlinkSync(lock);
    } catch {}
  }
  if (above) fenced(above);
}
if (durable) raiseFloor();

// A heartbeat line on stdout every CUA_SESSION_HEARTBEAT_SECS, from a timer of
// its own: the coordinator cancels a driver whose output stops growing, and a
// frozen process stops this timer. (Busy agents do not: they run in child
// processes.) 0 or unset: none.
const heartbeatSecs = Number(process.env.CUA_SESSION_HEARTBEAT_SECS ?? 0);
if (durable && heartbeatSecs > 0) {
  const beat = () => process.stdout.write(`CUA-HEARTBEAT ${durable.epoch} ${Date.now()}\n`);
  beat();
  setInterval(beat, heartbeatSecs * 1000).unref();
}

// One fenced, optionally fsynced append. Returns the file's size after it
// (the byte offset a ship ack must reach).
function appendJournal(file, line, sync) {
  checkFence();
  const fd = fs.openSync(J(file), "a", 0o600);
  try {
    fs.writeSync(fd, line + "\n");
    if (sync) fs.fsyncSync(fd);
    return fs.fstatSync(fd).size;
  } finally {
    fs.closeSync(fd);
  }
}

// ---- durable state ---------------------------------------------------------

function readLines(file) {
  let text = "";
  try {
    text = fs.readFileSync(J(file), "utf8");
  } catch {
    return [];
  }
  const out = [];
  for (const l of text.split("\n")) {
    if (!l) continue;
    try {
      out.push(JSON.parse(l));
    } catch {} // a torn last line
  }
  return out;
}

let seq = 0;
{
  const evs = readLines("events.jsonl");
  seq = evs.length ? (evs[evs.length - 1].seq ?? 0) : 0;
}
let prev = {};
try {
  prev = JSON.parse(fs.readFileSync(J("state.json"), "utf8"));
} catch {}
const state = {
  status: "starting",
  turn: prev.turn ?? 0,
  sessionId: prev.sessionId ?? null,
  runnerPid: process.pid,
  agentPid: null,
  stopReason: prev.stopReason ?? null,
  queued: 0,
  loadSession: prev.loadSession ?? false,
  modeId: prev.modeId ?? null,
  error: null,
  updatedAt: Date.now(),
  ...(durable ? { epoch: durable.epoch, degraded: false, agentRestarts: 0 } : {}),
};

function redact(s) {
  let out = s;
  for (const v of secretValues) out = out.split(v).join("[redacted]");
  return out;
}

const SYNC_EVENTS = new Set(["turn_started", "turn_ended", "session", "run_exited", "driver_start", "degraded"]);
function emit(type, fields = {}) {
  checkFence();
  seq += 1;
  const line = redact(
    JSON.stringify({ seq, ts: Date.now(), turn: state.turn, ...(durable ? { epoch: durable.epoch } : {}), type, ...fields }),
  );
  appendJournal("events.jsonl", line, Boolean(durable) && SYNC_EVENTS.has(type));
}

function save(patch = {}) {
  checkFence();
  Object.assign(state, patch, { updatedAt: Date.now() });
  const tmp = J(durable ? `.state.json.${process.pid}.tmp` : ".state.json.tmp");
  fs.writeFileSync(tmp, JSON.stringify(state));
  fs.renameSync(tmp, J("state.json"));
}

// ---- the tool ledger (durable) ---------------------------------------------------
//
// tools.jsonl, one record per line, all with {seq, ts, epoch, turn, kind,
// toolCallId}:
//   intent        {name, toolKind, digest, input, decision: "approve"}   fsync, before the answer
//   started       {name, toolKind, digest, unguarded: true}              a tool that asked no permission
//   outcome       {status: "completed" | "failed", summary, unguarded?, denied?}
//   may_have_run  {action: "deny", reason}   repeat request, outcome unknown
//   rerun         {policy: "rerun-safe"}     repeat of a read-only call, approved again
//   duplicate     {action: "deny"}           repeat request of a completed call

const READ_ONLY_KINDS = new Set(["read", "search", "fetch", "think", "switch_mode"]);
let lseq = 0;
const entries = []; // newest last
const byId = new Map(); // toolCallId -> newest entry
const toolInfo = new Map(); // toolCallId -> {name, toolKind, digest, input}

const digestOf = (v) => crypto.createHash("sha256").update(canonical(v ?? null)).digest("hex").slice(0, 32);
function canonical(v) {
  if (Array.isArray(v)) return `[${v.map(canonical).join(",")}]`;
  if (v && typeof v === "object") {
    return `{${Object.keys(v)
      .sort()
      .map((k) => `${JSON.stringify(k)}:${canonical(v[k])}`)
      .join(",")}}`;
  }
  return JSON.stringify(v);
}
const clip = (s, n) => (s.length > n ? s.slice(0, n - 1) + "…" : s);

function applyRecord(r) {
  let e = byId.get(r.toolCallId);
  if (r.kind === "intent" || r.kind === "started") {
    // A new call under a reused id starts a new entry once the old one ended.
    if (!e || e.outcome) {
      e = { id: r.toolCallId, firstSeq: r.seq };
      entries.push(e);
      byId.set(r.toolCallId, e);
    }
    Object.assign(e, { name: r.name, toolKind: r.toolKind, digest: r.digest, turn: r.turn, epoch: r.epoch });
    if (r.kind === "intent") {
      e.decision = r.decision;
      e.unguarded = false;
    } else if (!e.decision) e.unguarded = true;
  } else if (r.kind === "outcome") {
    if (!e) {
      e = { id: r.toolCallId, firstSeq: r.seq, name: r.name, unguarded: true, turn: r.turn };
      entries.push(e);
      byId.set(r.toolCallId, e);
    }
    e.outcome = { status: r.status, summary: r.summary, denied: r.denied };
  } else if (r.kind === "may_have_run" && e) {
    e.repeatRefused = (e.repeatRefused ?? 0) + 1;
  } else if (r.kind === "rerun" && e) {
    e.rerun = true;
  }
}
for (const r of readLines("tools.jsonl")) {
  lseq = Math.max(lseq, r.seq ?? 0);
  applyRecord(r);
}

function ledger(kind, toolCallId, fields = {}) {
  lseq += 1;
  const rec = { seq: lseq, ts: Date.now(), epoch: durable.epoch, turn: state.turn, kind, toolCallId, ...fields };
  const end = appendJournal("tools.jsonl", redact(JSON.stringify(rec)), true);
  applyRecord(rec);
  return { rec, end };
}

// ---- shipping (durable.ship) ------------------------------------------------------
//
// The journal is mirrored to the Cua Volume by the host (the runner has no
// Volume access inside the Space). For an intent the runner waits until the
// host acknowledges that tools.jsonl is on the Volume up to the intent's end:
// then a re-drive on any host sees the intent, and at-most-once holds across
// hosts. No ack in time: the call is refused and journaled as denied.
function shipAck() {
  try {
    return JSON.parse(fs.readFileSync(J(durable.ship.ackFile), "utf8"));
  } catch {
    return null;
  }
}

async function waitShipped(end) {
  const deadline = Date.now() + durable.ship.ackTimeoutMs;
  for (;;) {
    checkFence();
    const a = shipAck();
    if (a && Number(a.epoch) === durable.epoch && Number(a.end?.["tools.jsonl"] ?? 0) >= end) return true;
    if (Date.now() >= deadline) return false;
    await new Promise((r) => setTimeout(r, 50));
  }
}

// Some harnesses title a permission request with a generic label ("Run command") and put the command in the
// raw input (measured on codex-acp). The recovery note must say which command, so the label gets the command.
function toolLabel(title, raw) {
  const c = raw && typeof raw === "object" ? (Array.isArray(raw.command) ? raw.command.join(" ") : (raw.command ?? raw.cmd)) : undefined;
  if (typeof c !== "string" || !c || !title || title.includes(c)) return title;
  return `${title}: ${c.replace(/\s+/g, " ").trim()}`;
}

function noteTool(tc) {
  const old = toolInfo.get(tc.toolCallId) ?? {};
  const info = {
    name: toolLabel(tc.title, tc.rawInput) ?? old.name ?? tc.kind ?? "tool",
    toolKind: tc.kind ?? old.toolKind ?? "other",
    input: tc.rawInput !== undefined ? tc.rawInput : old.input,
  };
  info.digest = tc.rawInput !== undefined ? digestOf(tc.rawInput) : (old.digest ?? digestOf(info.name));
  toolInfo.set(tc.toolCallId, info);
  return info;
}

function resultSummary(update) {
  let s = "";
  const raw = update.rawOutput;
  if (raw !== undefined && raw !== null) s = typeof raw === "string" ? raw : JSON.stringify(raw);
  else {
    s = (update.content ?? [])
      .map((c) => (c?.content?.type === "text" ? c.content.text : c?.type === "diff" ? `diff ${c.path}` : ""))
      .filter(Boolean)
      .join(" ");
  }
  return clip(s.replace(/\s+/g, " ").trim(), 160);
}

// tool_call / tool_call_update from the agent -> ledger. A call that only
// announced itself (status pending) may still be waiting for permission and
// has run nothing; `started` is written once it is in progress or finished.
function ledgerUpdate(update) {
  const kind = update.sessionUpdate;
  if (kind !== "tool_call" && kind !== "tool_call_update") return;
  const id = update.toolCallId;
  const info = noteTool(update);
  const done = update.status === "completed" || update.status === "failed";
  const live = done || update.status === "in_progress";
  let e = byId.get(id);
  if (e?.refusalPending && update.status === "failed") {
    // The failure is our refusal of a repeat request, not the tool's result.
    e.refusalPending = false;
    return;
  }
  if ((!e || (e.outcome && kind === "tool_call")) && live) {
    ledger("started", id, { name: info.name, toolKind: info.toolKind, digest: info.digest, unguarded: true });
    e = byId.get(id);
  }
  if (done && e && !e.outcome) {
    ledger("outcome", id, {
      name: info.name,
      status: update.status,
      summary: resultSummary(update),
      ...(e.unguarded ? { unguarded: true } : {}),
    });
  }
}

// ---- the recovery note ---------------------------------------------------------

function userSays(prompt) {
  return clip(String(prompt ?? "").replace(/\s+/g, " ").trim(), 600);
}

// Pure: ledger entries + the journal facts -> the text the agent gets first.
export function buildRecoveryNote({ epoch, prevEpoch, interrupted, entries: es, degraded, history }) {
  const lines = [];
  lines.push(
    `[cua durable recovery] Your run was interrupted and has been resumed (epoch ${epoch}${prevEpoch ? `, previous epoch ${prevEpoch}` : ""}). This note is automatic; it is not from the user.`,
  );
  if (degraded) {
    lines.push(
      "Your conversation history could not be restored (this agent cannot reload a session), so you are starting with a fresh context. The journal summary below is all that is known.",
    );
  }
  if (interrupted) {
    lines.push("", `The request you were working on (turn ${interrupted.turn}) was:`, `> ${userSays(interrupted.prompt)}`);
  }
  const scope = degraded ? es : es.filter((e) => !interrupted || e.turn === undefined || e.turn >= interrupted.turn);
  const completed = scope.filter((e) => e.outcome && !e.outcome.denied);
  const unknown = scope.filter((e) => !e.outcome && (e.decision === "approve" || e.unguarded));
  if (completed.length) {
    lines.push("", "Tool calls that completed (do not repeat them):");
    for (const e of completed.slice(-40)) {
      lines.push(`- ${clip(e.name ?? "tool", 140)} [${e.outcome.status}]${e.outcome.summary ? `: ${e.outcome.summary}` : ""}`);
    }
    if (completed.length > 40) lines.push(`- (${completed.length - 40} earlier calls omitted)`);
  }
  if (unknown.length) {
    lines.push("", "Tool calls that may have partially run (started or approved, but no result was recorded). Verify the current state before repeating any of them:");
    for (const e of unknown.slice(-20)) {
      lines.push(
        `- ${clip(e.name ?? "tool", 140)} (id ${e.id})${e.repeatRefused ? "; a repeat of this call was refused after the restart" : ""}`,
      );
    }
  }
  if (!completed.length && !unknown.length) lines.push("", "No tool call is recorded for this request.");
  if (degraded && history) {
    if (history.prompts.length) {
      lines.push("", "Earlier requests in this run:");
      for (const p of history.prompts.slice(-8)) lines.push(`- ${userSays(p)}`);
    }
    if (history.lastAssistant) lines.push("", "Your last message before the interruption:", `> ${clip(history.lastAssistant.replace(/\s+/g, " ").trim(), 1500)}`);
  }
  lines.push("", interrupted ? "Continue the request from where it stopped." : "Carry on with the next request.");
  return lines.join("\n");
}

// What the journal says about the turns: the open (interrupted) one, the
// prompts that were queued but never started, and a short history.
function planFromJournal() {
  const evs = readLines("events.jsonl");
  let open = null;
  let lastEpoch = 0;
  let prevEpoch = 0;
  const started = new Set();
  const queued = new Map();
  const prompts = [];
  let lastAssistant = "";
  let sawStart = false;
  for (const e of evs) {
    lastEpoch = Math.max(lastEpoch, e.epoch ?? 0);
    if ((e.epoch ?? 0) < (durable?.epoch ?? Infinity)) prevEpoch = Math.max(prevEpoch, e.epoch ?? 0);
    if (e.type === "turn_started") {
      sawStart = true;
      if (e.msgId) started.add(e.msgId);
      const original = e.recovery ? (e.originalPrompt ?? open?.prompt ?? "") : e.prompt;
      open = { turn: e.recovery ? (e.resumesTurn ?? e.turn) : e.turn, prompt: original };
      if (!e.recovery) prompts.push(e.prompt);
    } else if (e.type === "turn_ended") {
      open = null;
    } else if (e.type === "prompt_queued") {
      queued.set(e.msgId, e);
    } else if (e.type === "update" && e.update?.sessionUpdate === "agent_message_chunk" && e.update.content?.text) {
      lastAssistant = e.update.content.text;
    }
  }
  const unstarted = [...queued.values()].filter((q) => !started.has(q.msgId));
  return { open, unstarted, sawStart, lastEpoch, prevEpoch, history: { prompts, lastAssistant }, hadJournal: evs.length > 0 };
}

// ---- update journaling ---------------------------------------------------------

// Consecutive text chunks of one kind become one event, flushed on a kind
// change, a non-chunk update, or after 250 ms.
let pending = null;
let pendingTimer = null;
function flush() {
  if (pendingTimer) clearTimeout(pendingTimer);
  pendingTimer = null;
  if (!pending) return;
  const p = pending;
  pending = null;
  emit("update", {
    update: { sessionUpdate: p.kind, content: { type: "text", text: p.text }, ...(p.messageId ? { messageId: p.messageId } : {}) },
  });
}
function onUpdate(update) {
  const kind = update.sessionUpdate;
  const chunk = (kind === "agent_message_chunk" || kind === "agent_thought_chunk" || kind === "user_message_chunk") && update.content?.type === "text";
  if (chunk) {
    if (pending && (pending.kind !== kind || pending.messageId !== update.messageId)) flush();
    if (!pending) pending = { kind, text: "", messageId: update.messageId };
    pending.text += update.content.text;
    if (!pendingTimer) pendingTimer = setTimeout(flush, 250);
    return;
  }
  flush();
  if (durable) ledgerUpdate(update);
  emit("update", { update });
}

// ---- the agent -------------------------------------------------------------

const env = { ...process.env, ...(cfg.env ?? {}), ...secrets };

// A rejected API key must not cost minutes. Claude Code retries a 401 ten
// times with growing pauses (about 3 minutes) and its ACP adapter reports
// nothing meanwhile, so the run reads "working" until the last retry fails.
// When the SDK names a cheap, read-only request for the run's key
// (cfg.keyCheck: url, headers with ${NAME} references into the run's env,
// keyEnv), ask the provider once, in parallel with the agent's start. Only
// a definitive "this key is invalid" (HTTP 401, or a 403 that says so, with
// the provider's JSON error body) counts: no key, a base-URL or sign-in
// override, a network error, a timeout, a 429 or a 5xx all resolve to null
// and the run goes on exactly as before, so the check can never block one.
const KEY_CHECK_TIMEOUT_MS = 3000;

function providerMessage(error) {
  let m = error?.message;
  if (typeof m !== "string") return "";
  // OpenAI echoes a masked copy of the key ("sk-proj-****abcd"): drop it.
  m = m.replace(/:?\s*\S*\*{3,}\S*/g, ".").replace(/\bsk-[\w-]{6,}/g, "").trim();
  return (m.split(/(?<=[.!?])\s+/)[0] ?? "").slice(0, 200);
}

async function checkKey(spec) {
  if (!spec?.url || !spec.keyEnv) return null;
  const key = env[spec.keyEnv];
  if (typeof key !== "string" || key.length === 0) return null;
  if ((spec.skipIfEnv ?? []).some((k) => env[k])) return null;
  for (const f of spec.skipIfFile ?? []) {
    try {
      if (fs.statSync(f.replace(/^~(?=\/)/, os.homedir())).size > 0) return null;
    } catch {}
  }
  const headers = {};
  for (const [k, v] of Object.entries(spec.headers ?? {})) {
    headers[k] = String(v).replace(/\$\{([A-Z0-9_]+)\}/g, (_, n) => env[n] ?? "");
  }
  try {
    const signal = AbortSignal.timeout(spec.timeoutMs ?? KEY_CHECK_TIMEOUT_MS);
    const res = await fetch(spec.url, { headers, redirect: "manual", signal });
    if (res.status !== 401 && res.status !== 403) return null;
    const error = JSON.parse(await res.text())?.error;
    if (!error || typeof error !== "object") return null;
    const invalid = error.type === "authentication_error" || error.code === "invalid_api_key";
    if (res.status === 403 && !invalid) return null;
    const why = providerMessage(error) || "the API key was rejected.";
    return { status: res.status, message: `Failed to authenticate. ${spec.provider ?? "Provider"} API Error: ${res.status} ${why}` };
  } catch {
    return null;
  }
}
const keyCheck = checkKey(cfg.keyCheck).catch(() => null);

const agentLog = fs.openSync(P("agent.log"), "a");
const logStart = fs.fstatSync(agentLog).size;

// Node reports a missing working folder, a missing program and a missing
// shebang interpreter all as `spawn <program> ENOENT`. Say which it is, in
// words a person can act on (the surfaces show this line first).
function spawnProblem(error) {
  if (error?.code !== "ENOENT" && error?.code !== "ENOTDIR") return "";
  let isDir = false;
  try {
    isDir = fs.statSync(cfg.cwd).isDirectory();
  } catch {}
  if (!isDir) {
    return `The working folder ${cfg.cwd} is not available in this Space (a folder on the Cua Volume is missing until the Volume is attached again).`;
  }
  let head = "";
  try {
    const fd = fs.openSync(command, "r");
    try {
      const b = Buffer.alloc(256);
      head = b.toString("utf8", 0, fs.readSync(fd, b, 0, 256, 0));
    } finally {
      fs.closeSync(fd);
    }
  } catch {
    return `${cfg.agent.command} is not installed in this Space.`;
  }
  const m = /^#!\s*(\S+)(?:\s+(\S+))?/.exec(head);
  if (m) {
    const prog = path.basename(m[1]) === "env" && m[2] ? m[2] : m[1];
    const found = prog.includes("/")
      ? fs.existsSync(prog)
      : (env.PATH ?? "").split(path.delimiter).some((d) => d && fs.existsSync(path.join(d, prog)));
    if (!found) return `${prog === "node" ? "Node" : prog} is not installed in this Space.`;
  }
  return "";
}

function diagnostic(error, phase) {
  const size = fs.fstatSync(agentLog).size;
  const tail = Buffer.alloc(Math.min(4096, size - logStart));
  const fd = fs.openSync(P("agent.log"), "r");
  try {
    fs.readSync(fd, tail, 0, tail.length, size - tail.length);
  } finally {
    fs.closeSync(fd);
  }
  // macOS reports a missing shebang interpreter as exit 127 from env, not ENOENT.
  const why = phase === "spawn" ? spawnProblem(error) : error?.exit127 ? spawnProblem({ code: "ENOENT" }) : "";
  return redact((why ? `${why}\n` : "")
    + `${error?.message ?? error}`
    + (error?.code != null ? ` (code ${error.code})` : "")
    + (error?.data != null ? ` ${JSON.stringify(error.data)}` : "")
    + ` (${phase}: ${cfg.agent.command})`
    + (error?.code === -32000 && cfg.authHint ? `\n${cfg.authHint}` : "")
    + (tail.length ? `\n${tail.toString("utf8").trim()}` : "")
    + (phase === "spawn" ? `\nPATH=${env.PATH ?? ""}` : "")
    + `\nAgent log: ${P("agent.log")}`);
}
// Resolve ~/.cua/bin links: some agents (Antigravity's .par) find their
// sibling files relative to argv[0].
let command = cfg.agent.command;
try {
  command = fs.realpathSync(command);
} catch {}

let exiting = false;
let gen = 0; // bumped for every agent process; stale loops stop
let restarts = 0;

// Starts the agent process. `gone` settles once, when it exits, fails to
// start, or closes its stdout (the ACP transport) while still alive.
function startAgent() {
  const myGen = ++gen;
  // The working folder can be gone when a run starts again (it was on a
  // Volume that is not attached yet): make it, or say it is not there.
  try {
    fs.mkdirSync(cfg.cwd, { recursive: true });
  } catch {}
  const c = spawn(command, cfg.agent.args ?? [], {
    cwd: cfg.cwd,
    env,
    stdio: ["pipe", "pipe", agentLog],
  });
  child = c;
  save({ agentPid: c.pid ?? null });
  emit("agent_spawned", { pid: c.pid ?? null });
  const gone = new Promise((resolve) => {
    let done = false;
    const fin = (v) => {
      if (!done) {
        done = true;
        resolve(v);
      }
    };
    c.on("error", (e) => fin({ error: e }));
    c.on("exit", (code, signal) => fin({ code, signal }));
    // A closed stdout ends the ACP connection even if the process lives on;
    // give a concurrent exit a moment to win so its code is reported.
    c.stdout?.on("end", () => setTimeout(() => fin({ stdoutClosed: true }), 250));
  });
  c.stdin.on("error", () => {}); // a dead agent: the exit is reported through `gone`
  return { c, gone, myGen };
}

// How a finished agent process reads in the run's error (with the agent.log tail).
function goneDiagnostic(g) {
  if (g.error) return diagnostic(g.error, "spawn");
  if (g.stdoutClosed) return diagnostic(new Error("agent stdout closed"), "transport");
  return diagnostic(Object.assign(new Error(`the agent exited unexpectedly (code ${g.code}, signal ${g.signal})`), { exit127: g.code === 127 }), "transport");
}

// End this run's cua-driver session on the guest MCP (cua-spacesd keys it by
// X-Cua-Agent-Session), so the run's presence cursor leaves at once instead
// of after the server's idle timeout. Best effort, bounded.
function endAgentSessions() {
  const expandSecret = (v) => (typeof v === "string" ? v.replace(/^\$\{([A-Z0-9_]+)\}$/, (_, k) => secrets[k] ?? process.env[k] ?? "") : v);
  const ends = [];
  for (const m of cfg.mcpServers ?? []) {
    if (m.type !== "http" || !m.url) continue;
    const headers = {};
    for (const h of m.headers ?? []) headers[h.name] = expandSecret(h.value);
    if (!Object.keys(headers).some((k) => k.toLowerCase() === "x-cua-agent-session")) continue;
    ends.push(
      fetch(m.url, { method: "DELETE", headers, signal: AbortSignal.timeout(1000) }).catch(() => {}),
    );
  }
  return Promise.allSettled(ends);
}

function endTurn(stopReason, usage = null) {
  if (exiting || state.status !== "running") return;
  flush();
  save({ status: "idle", stopReason, queued: queue.length });
  emit("turn_ended", { stopReason, usage });
}

function finish(code, why, message = null) {
  if (exiting) return;
  flush();
  if (message) emit("error", { message });
  // A durable run leaves its turn open: the re-driver (or the recovery
  // turn) resumes it.
  if (!durable) endTurn(code !== 0 ? "error" : why);
  exiting = true;
  process.exitCode = code; // even if the loop is gone and the event loop drains first
  const failed = code !== 0;
  emit("run_exited", { code, reason: why, resumable: Boolean(state.sessionId && state.loadSession) });
  save({ status: failed ? "failed" : "exited", error: failed ? redact(message ?? why) : null, exitCode: code });
  try {
    child?.kill("SIGTERM");
  } catch {}
  const exit = () => process.exit(code);
  setTimeout(exit, 1200).unref();
  endAgentSessions().finally(() => setTimeout(exit, 300).unref());
}
// A stopped durable runner is a crash to the re-driver (it never reports
// "done" because something killed it).
process.on("SIGTERM", () => finish(durable ? 143 : 0, "stopped"));
process.on("SIGINT", () => finish(durable ? 130 : 0, "stopped"));

// Poll the fence while idle: a stale runner stops even if nothing is written.
if (durable) setInterval(checkFence, 1000).unref();

function allowOption(options) {
  const by = (k) => options.find((o) => o.kind === k);
  // "Always" lets the harness stop asking for that kind of call, which would
  // leave it unguarded for the rest of the session.
  if (durable?.guard) return by("allow_once") ?? by("allow_always") ?? options[0];
  return by("allow_always") ?? by("allow_once") ?? options[0];
}
function rejectOption(options) {
  const by = (k) => options.find((o) => o.kind === k);
  return by("reject_once") ?? by("reject_always");
}

let replaying = false;

async function answerPermission(params) {
  const opts = params.options ?? [];
  const tc = params.toolCall ?? {};
  const allow = allowOption(opts);
  const reject = rejectOption(opts);
  const choose = (opt) => (opt ? { outcome: { outcome: "selected", optionId: opt.optionId } } : { outcome: { outcome: "cancelled" } });
  flush();
  if (!durable) {
    emit("permission", { toolCall: tc, chosen: allow?.optionId ?? null, kind: allow?.kind ?? null });
    return choose(allow);
  }
  const id = tc.toolCallId;
  const info = noteTool(tc);
  const e = byId.get(id);
  let action = "approve";
  let why = "";
  if (e && e.decision === "approve" && !e.outcome) {
    // The tool may be running, or may have half-run before a restart: never
    // approve it blindly a second time.
    if (durable.policy === "rerun-safe" && READ_ONLY_KINDS.has(info.toolKind)) {
      action = "rerun";
    } else {
      action = "deny";
      why = "repeat request for a tool call whose outcome is unknown";
      ledger("may_have_run", id, { name: info.name, action: "deny", reason: why });
      byId.get(id).refusalPending = true;
    }
  } else if (e && e.outcome && e.digest === info.digest && !e.outcome.denied) {
    action = "deny";
    why = "repeat request for a tool call that already completed";
    ledger("duplicate", id, { name: info.name, action: "deny" });
  }
  if (action === "approve" || action === "rerun") {
    if (action === "rerun") ledger("rerun", id, { name: info.name, policy: "rerun-safe" });
    else {
      const { end } = ledger("intent", id, {
        name: info.name,
        toolKind: info.toolKind,
        digest: info.digest,
        input: clip(JSON.stringify(info.input ?? null) ?? "null", 400),
        decision: "approve",
      });
      if (durable.ship && !(await waitShipped(end))) {
        why = "the journal did not reach the Cua Volume in time; the call was not run";
        ledger("outcome", id, { name: info.name, status: "failed", summary: why, denied: true });
        emit("permission", { toolCall: tc, chosen: reject?.optionId ?? null, kind: reject?.kind ?? null, action: "deny", reason: why });
        return choose(reject);
      }
    }
    emit("permission", { toolCall: tc, chosen: allow?.optionId ?? null, kind: allow?.kind ?? null, action });
    return choose(allow);
  }
  emit("permission", { toolCall: tc, chosen: reject?.optionId ?? null, kind: reject?.kind ?? null, action, reason: why });
  return choose(reject);
}

function makeClient(myGen) {
  return acp
    .client({ name: "cua-agent-runner" })
    .onRequest(acp.methods.client.session.requestPermission, (ctx) => answerPermission(ctx.params))
    .onNotification(acp.methods.client.session.update, (ctx) => {
      // History replayed by session/load is already in events.jsonl.
      if (!replaying && myGen === gen) onUpdate(ctx.params.update);
    });
}

// ---- inbox -------------------------------------------------------------------

const inbox = P("inbox");
fs.mkdirSync(path.join(inbox, "done"), { recursive: true });
const queue = [];

const plan = durable ? planFromJournal() : null;
if (durable) {
  // A prompt the journal never saw start (the first one, or an inbox message
  // taken just before a crash) is queued again; one that started is not.
  const first = [];
  if (cfg.prompt && !plan.sawStart) first.push({ op: "prompt", text: cfg.prompt, files: cfg.files ?? [], msgId: "initial" });
  for (const q of plan.unstarted) first.push({ op: "prompt", text: q.text, files: q.files ?? [], msgId: q.msgId });
  queue.push(...first);
} else if (cfg.prompt && !prev.sessionId) {
  queue.push({ op: "prompt", text: cfg.prompt, files: cfg.files ?? [] });
}

function drainInbox() {
  let names = [];
  try {
    names = fs.readdirSync(inbox).filter((n) => n.endsWith(".json") && !n.startsWith(".")).sort();
  } catch {
    return;
  }
  for (const n of names) {
    const f = path.join(inbox, n);
    let msg;
    try {
      msg = JSON.parse(fs.readFileSync(f, "utf8"));
    } catch {
      continue; // still being written
    }
    if (durable && msg.op === "prompt") {
      msg.msgId = `inbox-${n}`;
      emit("prompt_queued", { msgId: msg.msgId, text: msg.text, files: msg.files ?? [] });
    }
    fs.renameSync(f, path.join(inbox, "done", n));
    queue.push(msg);
  }
}

function blocks(text, files) {
  const out = [{ type: "text", text }];
  for (const f of files ?? []) out.push({ type: "resource_link", uri: `file://${f}`, name: path.basename(f) });
  return out;
}

// ---- main ------------------------------------------------------------------

class AgentFailure extends Error {
  constructor(message, permanent = false) {
    super(message);
    this.permanent = permanent;
  }
}
const AUTH_RE = /failed to authenticate|authentication required|invalid api key|invalid x-api-key|unauthorized/i;
const isAuthError = (e) => e?.code === -32000 || e?.data?.errorKind === "authentication_failed" || AUTH_RE.test(String(e?.message ?? ""));
const withAuthHint = (message, e) => message + (e?.code === -32000 && cfg.authHint ? `\n${cfg.authHint}` : "");

// The note for the next turn, when a journal says there is something to tell.
let pendingNote = null;

async function drive(a) {
  const { myGen } = a;
  const stale = () => exiting || myGen !== gen;
  const stream = acp.ndJsonStream(Writable.toWeb(a.c.stdin), Readable.toWeb(a.c.stdout));
  const client = makeClient(myGen);
  await client.connectWith(stream, async (ctx) => {
    // A startup request that never answers fails the run (or, durable,
    // restarts the agent) instead of leaving it "starting" forever. The
    // error carries its phase; a timeout is never swallowed by a fallback.
    const requestStartup = async (method, params, phase) => {
      let timer;
      try {
        return await Promise.race([
          ctx.request(method, params),
          new Promise((_, reject) => {
            timer = setTimeout(() => {
              reject(Object.assign(new Error(`${phase} timed out`), { timedOut: true }));
            }, cfg.startupTimeoutMs ?? 180000);
          }),
        ]);
      } catch (error) {
        if (error && typeof error === "object") error.phase ??= phase;
        throw error;
      } finally {
        clearTimeout(timer);
      }
    };
    const fatal = (e) => stale() || e?.timedOut || e?.code === -32000;
    const init = await requestStartup(acp.methods.agent.initialize, {
      protocolVersion: acp.PROTOCOL_VERSION,
      clientCapabilities: {},
      clientInfo: { name: "cua", version: "1" },
    }, "initialize");
    const caps = init.agentCapabilities ?? {};
    const canResume = Boolean(caps.sessionCapabilities?.resume);
    const canLoad = Boolean(caps.loadSession);
    const loadSession = canLoad || canResume;
    save({ loadSession });
    emit("initialized", { agentInfo: init.agentInfo ?? null, protocolVersion: init.protocolVersion, agentCapabilities: caps, authMethods: init.authMethods ?? [] });
    // Headless auth: the harness names the method its env credentials use
    // (for example "gemini-api-key"); agents that read env keys directly
    // need none.
    const method = (init.authMethods ?? []).find((m) => m.id === cfg.authMethod);
    if (method) {
      try {
        await requestStartup(acp.methods.agent.authenticate, { methodId: method.id }, "authenticate");
      } catch (e) {
        if (!durable || e?.timedOut) throw e;
        throw Object.assign(new AgentFailure(`authentication failed: ${e?.message ?? e}`, true), { code: e?.code, data: e?.data, phase: "authenticate" });
      }
      emit("authenticated", { methodId: method.id });
    }
    // HTTP MCP servers only for agents that declare mcpCapabilities.http
    // (stdio is the baseline every ACP agent supports).
    // Header and env values are ${NAME} references into secrets.json.
    const expand = (v) => (typeof v === "string" ? v.replace(/^\$\{([A-Z0-9_]+)\}$/, (_, k) => secrets[k] ?? process.env[k] ?? "") : v);
    // HTTP MCP servers go to agents that declare mcpCapabilities.http; the
    // rest get them through the mcp-remote stdio bridge (stdio is the
    // baseline every ACP agent supports).
    const mcpServers = [];
    for (const m of cfg.mcpServers ?? []) {
      for (const h of m.headers ?? []) h.value = expand(h.value);
      for (const e of m.env ?? []) e.value = expand(e.value);
      const native = (m.type === "http" && caps.mcpCapabilities?.http) || (m.type === "sse" && caps.mcpCapabilities?.sse);
      if (!m.type || native) {
        mcpServers.push(m);
      } else if (cfg.mcpBridge && m.type === "http") {
        const env = [];
        const args = [m.url, "--transport", "http-only", "--silent"];
        (m.headers ?? []).forEach((h, i) => {
          env.push({ name: `CUA_MCP_HEADER_${i}`, value: h.value });
          args.push("--header", `${h.name}:\${CUA_MCP_HEADER_${i}}`);
        });
        mcpServers.push({ name: m.name, command: cfg.mcpBridge, args, env });
        emit("notice", { message: `MCP server ${m.name} bridged to stdio (this agent has no HTTP MCP client)` });
      } else {
        emit("notice", { message: `MCP server ${m.name} skipped: this agent does not support ${m.type} MCP servers` });
      }
    }
    const request = { cwd: cfg.cwd, mcpServers };
    const wantSession = state.sessionId;
    let sessionId = null;
    let resumed = false;
    let modes = null;
    let degraded = false;
    if (wantSession && loadSession) {
      try {
        let res;
        if (canResume) {
          res = await requestStartup(acp.methods.agent.session.resume, { sessionId: wantSession, ...request }, "session resume");
        } else {
          replaying = true;
          res = await requestStartup(acp.methods.agent.session.load, { sessionId: wantSession, ...request }, "session load");
        }
        sessionId = wantSession;
        resumed = true;
        // The agent's mode comes back to its default on a resume (measured on codex-acp: "Approve for me",
        // whatever the first drive selected), so the runner picks the mode again from this response.
        modes = res?.modes ?? null;
      } catch (e) {
        if (fatal(e)) throw e;
        emit("notice", { message: `could not resume session ${wantSession}: ${e?.message ?? e}; starting a new one` });
      } finally {
        replaying = false;
      }
    }
    if (!sessionId) {
      if (durable && wantSession) {
        // The history is gone: the recovery note carries the journal instead.
        degraded = true;
        emit("degraded", {
          reason: loadSession ? "the agent could not reload the session" : "the agent supports neither session/resume nor session/load",
          lostSessionId: wantSession,
        });
      }
      let res;
      try {
        res = await requestStartup(acp.methods.agent.session.new, request, "session");
      } catch (e) {
        // Some agents reject session MCP servers they cannot start; keep
        // the run and say so rather than failing it.
        if (fatal(e) || !request.mcpServers.length) throw e;
        emit("notice", {
          message: `the agent refused the run's MCP servers (${e?.message ?? e}${e?.data ? ": " + JSON.stringify(e.data) : ""}); continuing without them`,
        });
        request.mcpServers = [];
        res = await requestStartup(acp.methods.agent.session.new, request, "session");
      }
      sessionId = res.sessionId;
      modes = res.modes ?? null;
    }
    if (stale()) return;
    save({ sessionId, status: "idle", ...(durable ? { degraded: state.degraded || degraded } : {}) });
    emit("session", { sessionId, resumed, modes });

    // What the journal says happened before this agent process (re-read: an
    // in-process restart sees the turn it just lost).
    if (durable) {
      const p = planFromJournal();
      const es = entries.slice();
      const hasUnknown = es.some((e) => !e.outcome && (e.decision === "approve" || e.unguarded));
      const base = { epoch: durable.epoch, prevEpoch: p.prevEpoch, entries: es, degraded, history: p.history };
      if (p.open) {
        pendingNote = { recovery: true, interrupted: p.open, text: buildRecoveryNote({ ...base, interrupted: p.open }) };
      } else if (degraded || hasUnknown) {
        pendingNote = { recovery: false, text: buildRecoveryNote({ ...base, interrupted: null }) };
      }
    }

    // The sandbox is the boundary: pick the agent's own "full access" mode
    // when it has one (by `_meta.kind`, else by id), so its tools are not
    // sandboxed a second time inside the sandbox.
    // Guard mode keeps the harness asking: it selects the harness's own
    // ask-for-approval mode when its default does not ask (Codex), else none.
    const want = (durable?.guard ? cfg.guardModeKinds : cfg.modeKinds) ?? [];
    const mode = (modes?.availableModes ?? []).find((m) => want.includes(m?._meta?.kind) || want.includes(m.id));
    // A resume response without modes: select the mode the first drive chose (kept in state.json).
    const modeId = mode?.id ?? (resumed && !modes?.availableModes ? state.modeId : undefined);
    if (modeId && modeId !== modes?.currentModeId) {
      try {
        await ctx.request(acp.methods.agent.session.setMode, { sessionId, modeId });
        save({ modeId });
        emit("mode", { modeId, ...(resumed ? { resumed: true } : {}) });
      } catch (e) {
        emit("notice", { message: `could not switch to mode ${modeId}: ${e?.message ?? e}` });
      }
    }

    const promptTurn = async (msg) => {
      state.turn += 1;
      let text = msg.text;
      let extra = {};
      if (durable && pendingNote) {
        const n = pendingNote;
        pendingNote = null;
        if (n.recovery) {
          // The resumed turn's first prompt is the recovery note; the
          // interrupted request is not sent again.
          text = n.text;
          extra = { recovery: true, resumesTurn: n.interrupted.turn, originalPrompt: n.interrupted.prompt };
        } else {
          text = `${n.text}\n\n${msg.text}`;
          extra = { noteAttached: true };
        }
      }
      save({ status: "running", queued: queue.length });
      emit("turn_started", { prompt: text, files: msg.files ?? [], ...(msg.msgId ? { msgId: msg.msgId } : {}), ...extra });
      const rejected = await keyCheck;
      if (stale()) return;
      if (rejected) {
        // The same record a failed prompt leaves, without the harness's
        // retries: one error, the turn ends, the supervisor notifies once.
        emit("error", { message: rejected.message, data: { errorKind: "authentication_failed", status: rejected.status, source: "key_check" } });
        endTurn("error");
        // A run whose credential is rejected is over: the next message starts
        // a fresh run with whatever credential the person has connected by then.
        finish(durable ? EXIT_PERMANENT : 1, withAuthHint(rejected.message, null));
        return;
      }
      let res;
      try {
        res = await ctx.request(acp.methods.agent.session.prompt, { sessionId, prompt: blocks(text, msg.files) });
      } catch (e) {
        if (stale()) return; // the agent died; the turn stays open for the recovery note
        flush();
        emit("error", { message: withAuthHint(e?.message ?? String(e), e), code: e?.code ?? null, data: e?.data ?? null });
        if (durable) {
          // A failed turn is not a finished one: leave it open and let the
          // restart (or the re-driver) resume it.
          throw new AgentFailure(`prompt failed: ${e?.message ?? e}`, isAuthError(e));
        }
        endTurn("error");
        // Same for a credential the agent says it could not authenticate: the
        // run ends as failed (its error kept) instead of idling for up to
        // 30 minutes with a credential that does not work.
        if (isAuthError(e)) finish(1, redact(withAuthHint(e?.message ?? String(e), e)));
        return;
      }
      if (stale()) return;
      endTurn(res?.stopReason ?? null, res?.usage ?? null);
    };

    let idleSince = Date.now();
    const idleExitMs = (cfg.idleExitSecs ?? 1800) * 1000;
    const exitWhenIdle = cfg.exitWhenIdle ?? Boolean(durable);
    let running = null;
    let failure = null;
    // An interrupted turn runs first, as a recovery turn.
    if (durable && pendingNote?.recovery) queue.unshift({ op: "prompt", text: "", files: [], msgId: `recovery-${durable.epoch}-${seq}`, recovery: true });
    for (;;) {
      if (stale()) return;
      if (failure) throw failure;
      drainInbox();
      while (queue.length && queue[0].op !== "prompt") {
        const m = queue.shift();
        if (m.op === "cancel") {
          if (running) {
            emit("cancel_requested", {});
            await ctx.notify(acp.methods.agent.session.cancel, { sessionId });
          }
        } else if (m.op === "stop") {
          if (running) {
            await ctx.notify(acp.methods.agent.session.cancel, { sessionId });
            await Promise.race([running, new Promise((r) => setTimeout(r, 5000))]);
          }
          finish(0, "stopped");
          return;
        }
      }
      if (!running && queue.length) {
        const m = queue.shift();
        running = promptTurn(m)
          .catch((e) => {
            failure = e;
          })
          .finally(() => {
            running = null;
            idleSince = Date.now();
          });
      }
      if (!running && !failure && exitWhenIdle && queue.length === 0) {
        finish(0, "done");
        return;
      }
      if (!running && Date.now() - idleSince > idleExitMs) {
        finish(0, "idle timeout");
        return;
      }
      await new Promise((r) => setTimeout(r, 200));
    }
  });
}

async function main() {
  emit("runner_started", {});
  if (durable) {
    emit("driver_start", {
      session: durable.session,
      attempt: durable.attempt,
      resume: durable.resume,
      journal: plan.hadJournal,
      policy: durable.policy,
      space: durable.space || undefined,
    });
    if (durable.resume && !plan.hadJournal) {
      emit("notice", { message: "resume was requested but there is no journal in the state directory; starting fresh" });
    }
    // Nothing left to do (every turn ended, nothing queued): do not even
    // start the agent, and never replay a finished turn.
    if (plan.sawStart && !plan.open && queue.length === 0 && (cfg.exitWhenIdle ?? true)) {
      finish(0, "done");
      return;
    }
  }
  for (;;) {
    let a;
    try {
      a = startAgent();
    } catch (e) {
      finish(durable ? EXIT_PERMANENT : 127, "could not start the agent", diagnostic(e, "spawn"));
      return;
    }
    let outcome;
    try {
      outcome = await Promise.race([drive(a).then(() => ({ ended: true })), a.gone.then((g) => ({ gone: g }))]);
    } catch (e) {
      outcome = { failure: e };
    }
    if (exiting) return;
    // The connection ended without the runner finishing: it ended because
    // the agent process did.
    if (outcome.ended) outcome = { gone: await a.gone };
    flush();
    if (outcome.gone?.error) {
      finish(durable ? EXIT_PERMANENT : 127, "could not start the agent", goneDiagnostic(outcome.gone));
      return;
    }
    const failure = outcome.failure;
    // The run's error: what happened, the agent.log tail, and (for -32000)
    // the sign-in hint.
    const diag = outcome.gone ? goneDiagnostic(outcome.gone) : diagnostic(failure, failure?.phase ?? "ACP connection");
    if (!durable) {
      if (outcome.gone?.stdoutClosed) finish(1, "ACP stdout closed", diag);
      else if (outcome.gone) finish(outcome.gone.code || 1, "agent exited", diag);
      else finish(1, failure?.timedOut ? failure.message : "ACP connection failed", diag);
      return;
    }
    // Durable: the agent crashed or its connection failed.
    const why = outcome.gone?.stdoutClosed
      ? "the agent closed its stdout (the ACP connection)"
      : outcome.gone
        ? `the agent exited unexpectedly (code ${outcome.gone.code}, signal ${outcome.gone.signal})`
        : failure?.timedOut
          ? failure.message
          : `ACP connection failed: ${failure?.message ?? failure}`;
    const permanent = failure instanceof AgentFailure ? failure.permanent : failure ? isAuthError(failure) : false;
    if (!failure?.message?.startsWith?.("prompt failed")) emit("error", { message: outcome.gone ? diag : `${why}\n${diag}`, data: failure?.data ?? null });
    if (permanent) {
      finish(EXIT_PERMANENT, why);
      return;
    }
    try {
      child?.kill("SIGKILL");
    } catch {}
    if (restarts >= durable.maxAgentRestarts) {
      finish(outcome.gone?.code || 1, `${why}; gave up after ${restarts} restart${restarts === 1 ? "" : "s"}`);
      return;
    }
    restarts += 1;
    gen += 1; // stale loops of the dead agent stop
    emit("agent_restart", { n: restarts, of: durable.maxAgentRestarts, reason: why });
    save({ agentRestarts: restarts, status: "starting" });
    await new Promise((r) => setTimeout(r, Math.min(250 * restarts, 2000)));
  }
}

main().catch((e) => {
  try {
    emit("error", { message: `runner failed: ${e?.message ?? e}` });
  } catch {}
  finish(1, `runner failed: ${e?.message ?? e}`);
});
