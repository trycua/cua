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

import * as acp from "@agentclientprotocol/sdk";
import { spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { Readable, Writable } from "node:stream";

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
const secretValues = Object.values(secrets).filter((v) => typeof v === "string" && v.length >= 8);

// ---- durable state ---------------------------------------------------------

let seq = 0;
try {
  const lines = fs.readFileSync(P("events.jsonl"), "utf8").trim().split("\n");
  const last = lines.length ? JSON.parse(lines[lines.length - 1]) : null;
  seq = last?.seq ?? 0;
} catch {}
let prev = {};
try {
  prev = JSON.parse(fs.readFileSync(P("state.json"), "utf8"));
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
  error: null,
  updatedAt: Date.now(),
};

function redact(s) {
  let out = s;
  for (const v of secretValues) out = out.split(v).join("[redacted]");
  return out;
}

function emit(type, fields = {}) {
  seq += 1;
  const line = redact(JSON.stringify({ seq, ts: Date.now(), turn: state.turn, type, ...fields }));
  fs.appendFileSync(P("events.jsonl"), line + "\n");
}

function save(patch = {}) {
  Object.assign(state, patch, { updatedAt: Date.now() });
  const tmp = P(".state.json.tmp");
  fs.writeFileSync(tmp, JSON.stringify(state));
  fs.renameSync(tmp, P("state.json"));
}

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
  emit("update", { update });
}

// ---- the agent -------------------------------------------------------------

const env = { ...process.env, ...(cfg.env ?? {}), ...secrets };
const agentLog = fs.openSync(P("agent.log"), "a");
// Resolve ~/.cua/bin links: some agents (Antigravity's .par) find their
// sibling files relative to argv[0].
let command = cfg.agent.command;
try {
  command = fs.realpathSync(command);
} catch {}
const child = spawn(command, cfg.agent.args ?? [], {
  cwd: cfg.cwd,
  env,
  stdio: ["pipe", "pipe", agentLog],
});
save({ agentPid: child.pid ?? null });
let exiting = false;
child.on("error", (e) => {
  emit("error", { message: `could not start the agent: ${e.message}` });
  finish(127, `could not start the agent: ${e.message}`);
});
child.on("exit", (code, signal) => {
  if (!exiting) {
    flush();
    emit("error", { message: `the agent exited unexpectedly (code ${code}, signal ${signal})` });
    finish(code ?? 1, "agent exited");
  }
});

// End this run's cua-driver session on the guest MCP (cua-spacesd keys it by
// X-Cua-Agent-Session), so the run's presence cursor leaves at once instead
// of after the server's idle timeout. Best effort, bounded.
function endAgentSessions() {
  const expandSecret = (v) => (typeof v === "string" ? v.replace(/^\$\{([A-Z0-9_]+)\}$/, (_, k) => secrets[k] ?? "") : v);
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

function finish(code, why) {
  if (exiting) return;
  exiting = true;
  flush();
  const failed = code !== 0;
  emit("run_exited", { code, reason: why, resumable: Boolean(state.sessionId && state.loadSession) });
  save({ status: failed ? "failed" : "exited", error: failed ? why : null });
  try {
    child.kill("SIGTERM");
  } catch {}
  const exit = () => process.exit(code);
  setTimeout(exit, 1200).unref();
  endAgentSessions().finally(() => setTimeout(exit, 300).unref());
}
process.on("SIGTERM", () => finish(0, "stopped"));
process.on("SIGINT", () => finish(0, "stopped"));

const stream = acp.ndJsonStream(Writable.toWeb(child.stdin), Readable.toWeb(child.stdout));

function allowOption(options) {
  const by = (k) => options.find((o) => o.kind === k);
  return by("allow_always") ?? by("allow_once") ?? options[0];
}

let replaying = false;
const client = acp
  .client({ name: "cua-agent-runner" })
  .onRequest(acp.methods.client.session.requestPermission, (ctx) => {
    const opt = allowOption(ctx.params.options);
    flush();
    emit("permission", { toolCall: ctx.params.toolCall, chosen: opt?.optionId ?? null, kind: opt?.kind ?? null });
    return opt ? { outcome: { outcome: "selected", optionId: opt.optionId } } : { outcome: { outcome: "cancelled" } };
  })
  .onNotification(acp.methods.client.session.update, (ctx) => {
    // History replayed by session/load is already in events.jsonl.
    if (!replaying) onUpdate(ctx.params.update);
  });

// ---- inbox -------------------------------------------------------------------

const inbox = P("inbox");
fs.mkdirSync(path.join(inbox, "done"), { recursive: true });
const queue = [];
if (cfg.prompt && !prev.sessionId) queue.push({ op: "prompt", text: cfg.prompt, files: cfg.files ?? [] });

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

await client
  .connectWith(stream, async (ctx) => {
    const init = await ctx.request(acp.methods.agent.initialize, {
      protocolVersion: acp.PROTOCOL_VERSION,
      clientCapabilities: {},
      clientInfo: { name: "cua", version: "1" },
    });
    const caps = init.agentCapabilities ?? {};
    const loadSession = Boolean(caps.loadSession || caps.sessionCapabilities?.resume);
    save({ loadSession });
    emit("initialized", { agentInfo: init.agentInfo ?? null, protocolVersion: init.protocolVersion, agentCapabilities: caps, authMethods: init.authMethods ?? [] });
    // Headless auth: the harness names the method its env credentials use
    // (for example "gemini-api-key"); agents that read env keys directly
    // need none.
    const method = (init.authMethods ?? []).find((m) => m.id === cfg.authMethod);
    if (method) {
      await ctx.request(acp.methods.agent.authenticate, { methodId: method.id });
      emit("authenticated", { methodId: method.id });
    }
    // HTTP MCP servers only for agents that declare mcpCapabilities.http
    // (stdio is the baseline every ACP agent supports).
    // Header and env values are ${NAME} references into secrets.json.
    const expand = (v) => (typeof v === "string" ? v.replace(/^\$\{([A-Z0-9_]+)\}$/, (_, k) => secrets[k] ?? "") : v);
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
    let sessionId = null;
    let resumed = false;
    let modes = null;
    if (prev.sessionId && loadSession) {
      try {
        if (caps.sessionCapabilities?.resume) {
          await ctx.request(acp.methods.agent.session.resume, { sessionId: prev.sessionId, ...request });
        } else {
          replaying = true;
          await ctx.request(acp.methods.agent.session.load, { sessionId: prev.sessionId, ...request });
        }
        sessionId = prev.sessionId;
        resumed = true;
      } catch (e) {
        emit("notice", { message: `could not resume session ${prev.sessionId}: ${e?.message ?? e}; starting a new one` });
      } finally {
        replaying = false;
      }
    }
    if (!sessionId) {
      let res;
      try {
        res = await ctx.request(acp.methods.agent.session.new, request);
      } catch (e) {
        // Some agents reject session MCP servers they cannot start; keep
        // the run and say so rather than failing it.
        if (!request.mcpServers.length) throw e;
        emit("notice", {
          message: `the agent refused the run's MCP servers (${e?.message ?? e}${e?.data ? ": " + JSON.stringify(e.data) : ""}); continuing without them`,
        });
        request.mcpServers = [];
        res = await ctx.request(acp.methods.agent.session.new, request);
      }
      sessionId = res.sessionId;
      modes = res.modes ?? null;
    }
    save({ sessionId, status: "idle" });
    emit("session", { sessionId, resumed, modes });
    // The sandbox is the boundary: pick the agent's own "full access" mode
    // when it has one (by `_meta.kind`, else by id), so its tools are not
    // sandboxed a second time inside the sandbox.
    const want = cfg.modeKinds ?? [];
    const mode = (modes?.availableModes ?? []).find((m) => want.includes(m?._meta?.kind) || want.includes(m.id));
    if (mode && mode.id !== modes?.currentModeId) {
      try {
        await ctx.request(acp.methods.agent.session.setMode, { sessionId, modeId: mode.id });
        emit("mode", { modeId: mode.id });
      } catch (e) {
        emit("notice", { message: `could not switch to mode ${mode.id}: ${e?.message ?? e}` });
      }
    }

    const promptTurn = async (msg) => {
      state.turn += 1;
      save({ status: "running", queued: queue.length });
      emit("turn_started", { prompt: msg.text, files: msg.files ?? [] });
      let res;
      try {
        res = await ctx.request(acp.methods.agent.session.prompt, { sessionId, prompt: blocks(msg.text, msg.files) });
      } catch (e) {
        flush();
        emit("error", { message: e?.message ?? String(e), data: e?.data ?? null });
        save({ status: "idle", stopReason: "error" });
        emit("turn_ended", { stopReason: "error" });
        return;
      }
      flush();
      save({ status: "idle", stopReason: res?.stopReason ?? null, queued: queue.length });
      emit("turn_ended", { stopReason: res?.stopReason ?? null, usage: res?.usage ?? null });
    };

    let idleSince = Date.now();
    const idleExitMs = (cfg.idleExitSecs ?? 1800) * 1000;
    let running = null;
    for (;;) {
      if (exiting) return;
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
        running = promptTurn(m).finally(() => {
          running = null;
          idleSince = Date.now();
        });
      }
      if (!running && cfg.exitWhenIdle && queue.length === 0) {
        finish(0, "done");
        return;
      }
      if (!running && Date.now() - idleSince > idleExitMs) {
        finish(0, "idle timeout");
        return;
      }
      await new Promise((r) => setTimeout(r, 200));
    }
  })
  .catch((e) => {
    emit("error", { message: `ACP connection failed: ${e?.message ?? e}`, data: e?.data ?? null });
    finish(1, `ACP connection failed: ${e?.message ?? e}`);
  });
