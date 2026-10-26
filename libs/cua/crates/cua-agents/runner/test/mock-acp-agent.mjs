// A scripted ACP agent (server side of @agentclientprotocol/sdk) for the
// durable runner's tests. Not a model: it does exactly what its script says.
//
//   MOCK_DIR     scratch dir: sessions.json (known sessions), log.jsonl (what
//                the agent saw), effects.log (one line per executed tool),
//                marks/ (once-only markers)
//   MOCK_SCRIPT  path to JSON: {"loadSession": bool, "resume": bool,
//                "prompts": [{"match": "regex", "steps": [...]}]}; the first
//                entry whose regex matches the prompt text runs
//
// Steps: {say}, {tool: {id,title,kind,input,permission,effect,fail}},
//        {crash, once}, {killRunner, once}, {hold}, {pause}, {startsOnly}
import * as acp from "@agentclientprotocol/sdk";
import fs from "node:fs";
import path from "node:path";
import { Readable, Writable } from "node:stream";

const dir = process.env.MOCK_DIR;
const script = JSON.parse(fs.readFileSync(process.env.MOCK_SCRIPT, "utf8"));
const log = (rec) => fs.appendFileSync(path.join(dir, "log.jsonl"), JSON.stringify({ pid: process.pid, ...rec }) + "\n");
const sessionsFile = path.join(dir, "sessions.json");
const sessions = () => {
  try {
    return JSON.parse(fs.readFileSync(sessionsFile, "utf8"));
  } catch {
    return {};
  }
};
const marks = path.join(dir, "marks");
fs.mkdirSync(marks, { recursive: true });
const once = (k) => {
  const f = path.join(marks, k);
  if (fs.existsSync(f)) return false;
  fs.writeFileSync(f, "");
  return true;
};
const ppid = process.ppid;
setInterval(() => {
  if (process.ppid !== ppid) process.exit(0); // the runner is gone
}, 200).unref();
log({ ev: "start", epochEnv: process.env.CUA_SESSION_EPOCH ?? null, secret: process.env.MOCK_SECRET ?? null });

// Notifications are fire-and-forget: let the runner read them before it dies.
const settle = () => new Promise((r) => setTimeout(r, 300));

async function runSteps(steps, sid, cx) {
  const note = (update) => cx.notify("session/update", { sessionId: sid, update });
  for (const st of steps) {
    if (st.say) await note({ sessionUpdate: "agent_message_chunk", content: { type: "text", text: st.say } });
    if (st.pause) await new Promise((r) => setTimeout(r, st.pause));
    if (st.crash && (!st.once || once(st.once))) process.exit(st.code ?? 1);
    if (st.killRunner && (!st.once || once(st.once))) {
      await settle();
      process.kill(process.ppid, "SIGKILL");
      await new Promise((r) => setTimeout(r, 5000));
    }
    if (st.hold) await new Promise(() => {});
    if (st.tool) {
      const t = st.tool;
      const base = { toolCallId: t.id, title: t.title ?? t.id, kind: t.kind ?? "execute", rawInput: t.input ?? { id: t.id } };
      await note({ sessionUpdate: "tool_call", ...base, status: "pending" });
      if (t.permission !== false) {
        const res = await cx.request("session/request_permission", {
          sessionId: sid,
          toolCall: { ...base, status: "pending" },
          options: [
            { kind: "allow_always", name: "Always allow", optionId: "always" },
            { kind: "allow_once", name: "Allow", optionId: "allow" },
            { kind: "reject_once", name: "Reject", optionId: "reject" },
          ],
        });
        const intentSeen = (() => {
          try {
            return fs
              .readFileSync(path.join(process.env.MOCK_STATE_DIR, "tools.jsonl"), "utf8")
              .split("\n")
              .filter(Boolean)
              .map((l) => JSON.parse(l))
              .some((r) => r.kind === "intent" && r.toolCallId === t.id);
          } catch {
            return false;
          }
        })();
        log({ ev: "permission", id: t.id, outcome: res.outcome, intentSeenAtAnswer: intentSeen });
        if (res.outcome.outcome !== "selected" || res.outcome.optionId === "reject") {
          await note({ sessionUpdate: "tool_call_update", toolCallId: t.id, status: "failed", rawOutput: "rejected" });
          continue;
        }
      }
      await note({ sessionUpdate: "tool_call_update", toolCallId: t.id, status: "in_progress" });
      fs.appendFileSync(path.join(dir, "effects.log"), `${t.id}\n`);
      if (t.pause) await new Promise((r) => setTimeout(r, t.pause));
      if (t.killRunnerBeforeOutcome && once(`kb-${t.id}`)) {
        await settle();
        process.kill(process.ppid, "SIGKILL");
        await new Promise((r) => setTimeout(r, 5000));
      }
      if (t.crashBeforeOutcome && once(`cb-${t.id}`)) process.exit(1);
      await note({
        sessionUpdate: "tool_call_update",
        toolCallId: t.id,
        status: t.fail ? "failed" : "completed",
        rawOutput: t.output ?? `ok ${t.id}`,
      });
    }
  }
}

const agent = acp.agent({ name: "mock-agent" });
agent
  .onRequest("initialize", () => ({
    protocolVersion: acp.PROTOCOL_VERSION,
    agentCapabilities: {
      loadSession: script.loadSession !== false,
      ...(script.resume ? { sessionCapabilities: { resume: {} } } : {}),
    },
    authMethods: [],
  }))
  .onRequest("session/new", () => {
    const id = `mock-${Math.random().toString(16).slice(2, 10)}`;
    fs.writeFileSync(sessionsFile, JSON.stringify({ ...sessions(), [id]: true }));
    log({ ev: "session_new", id });
    return {
      sessionId: id,
      modes: { currentModeId: "default", availableModes: [{ id: "default" }, { id: "ask", _meta: { kind: "standard" } }, { id: "full", _meta: { kind: "full_access" } }] },
    };
  })
  .onRequest("session/set_mode", (ctx) => {
    log({ ev: "set_mode", modeId: ctx.params.modeId });
    return {};
  })
  .onRequest("session/load", (ctx) => {
    if (!sessions()[ctx.params.sessionId]) throw new Error("unknown session");
    log({ ev: "session_load", id: ctx.params.sessionId });
    return {};
  })
  .onRequest("session/resume", (ctx) => {
    if (!sessions()[ctx.params.sessionId]) throw new Error("unknown session");
    log({ ev: "session_resume", id: ctx.params.sessionId });
    return {};
  })
  .onRequest("session/prompt", async (ctx) => {
    const text = ctx.params.prompt.map((b) => b.text ?? "").join("\n");
    log({ ev: "prompt", text });
    const entry = script.prompts.find((p) => new RegExp(p.match, "s").test(text));
    if (script.promptError && (!script.promptError.match || new RegExp(script.promptError.match).test(text))) {
      throw script.promptError.code === -32000 ? acp.RequestError.authRequired(undefined, script.promptError.message) : new Error(script.promptError.message);
    }
    await runSteps(entry?.steps ?? [], ctx.params.sessionId, ctx.client);
    return { stopReason: "end_turn" };
  })
  .onNotification("session/cancel", () => {})
  .connect(acp.ndJsonStream(Writable.toWeb(process.stdout), Readable.toWeb(process.stdin)));
