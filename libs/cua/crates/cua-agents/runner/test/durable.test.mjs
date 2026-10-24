// The durable ACP runner against a scripted mock ACP agent
// (mock-acp-agent.mjs): the epoch fence, the tool ledger, recovery notes and
// agent restarts. No model, no network. Needs node >= 22; the pinned
// @agentclientprotocol/sdk comes from $ACP_SDK_DIR, ~/.cua/tools/acp-sdk, or
// is installed once into the temp dir (the tests skip, saying so, without it).
//
//   node --test libs/cua/crates/cua-agents/runner/test/
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { Box, sdkRoot } from "./helpers.mjs";

const sdk = sdkRoot();
const t = (name, fn) => test(name, { skip: sdk ? false : "@agentclientprotocol/sdk is not available", timeout: 60_000 }, () => withBox(fn));

async function withBox(fn) {
  const box = new Box(sdk);
  try {
    await fn(box);
  } finally {
    box.cleanup();
  }
}

const tool = (id, extra = {}) => ({ tool: { id, title: `run ${id}`, kind: "execute", input: { cmd: id }, ...extra } });
const floor = (box) => Number(fs.readFileSync(path.join(box.state, "epoch"), "utf8"));

// ---- the fence -----------------------------------------------------------

t("a stale epoch is refused: exit 75, nothing written, the agent never starts", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [{ say: "hi" }] }] });
  box.config();
  fs.mkdirSync(box.state, { recursive: true });
  fs.writeFileSync(path.join(box.state, "epoch"), "5");
  const r = await box.go({ epoch: 3 });
  assert.equal(r.code, 75);
  assert.deepEqual(fs.readdirSync(box.state).sort(), ["epoch"]);
  assert.equal(floor(box), 5);
  assert.equal(box.agentLog().length, 0);
});

t("the new epoch raises the floor; a running older runner stops with 75 and writes nothing more", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [{ say: "working" }, { hold: true }] }] });
  box.config();
  const old = box.go({ epoch: 1 });
  // wait until the old driver is mid-turn
  for (let i = 0; i < 100 && !box.events().some((e) => e.type === "turn_started"); i++) await new Promise((r) => setTimeout(r, 100));
  assert.equal(floor(box), 1);
  fs.writeFileSync(path.join(box.state, "epoch"), "2"); // what a newer driver's raise leaves
  const r = await old;
  assert.equal(r.code, 75);
  const sizes = ["events.jsonl", "tools.jsonl", "state.json"].map((f) => fs.existsSync(path.join(box.state, f)) && fs.statSync(path.join(box.state, f)).size);
  await new Promise((r) => setTimeout(r, 600));
  assert.deepEqual(["events.jsonl", "tools.jsonl", "state.json"].map((f) => fs.existsSync(path.join(box.state, f)) && fs.statSync(path.join(box.state, f)).size), sizes);
  assert.equal(box.events().filter((e) => e.type === "run_exited").length, 0, "a fenced runner reports nothing");
  assert.equal(floor(box), 2);
});

t("concurrent starts never lower the floor", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [{ say: "x" }] }] });
  box.config();
  const rs = await Promise.all([2, 4, 3, 1].map((epoch) => box.go({ epoch })));
  for (const [i, r] of rs.entries()) assert.ok([0, 75].includes(r.code), `runner ${i}: ${r.code} ${r.stderr}`);
  assert.equal(rs[0].code === 0 || rs[0].code === 75, true);
  assert.equal(floor(box), 4);
});

t("an empty state dir falls back to $CUA_JOB_DATA/session", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [{ say: "x" }] }] });
  box.config({ durable: { stateDir: "" } });
  const job = path.join(box.root, "job");
  const r = await box.go({ epoch: 1, resume: false, env: { CUA_SESSION_STATE_DIR: "", CUA_JOB_DATA: job } });
  assert.equal(r.code, 0, r.stderr);
  assert.equal(fs.readFileSync(path.join(job, "session", "epoch"), "utf8"), "1");
  assert.ok(box.lines("events.jsonl", path.join(job, "session")).length > 0);
});

t("no state dir at all is a permanent failure", async (box) => {
  box.script({ prompts: [] });
  box.config({ durable: { stateDir: "" } });
  assert.equal((await box.go({ epoch: 1, env: { CUA_SESSION_STATE_DIR: "", CUA_JOB_DATA: "" } })).code, 78);
});

// ---- intent before approve, outcomes, recovery ------------------------------

t("the intent is on disk before the approval is answered; one approval per call id", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [tool("call-1"), { say: "done" }] }] });
  box.config();
  const r = await box.go({ epoch: 1, resume: false });
  assert.equal(r.code, 0, r.stderr);
  const perm = box.agentLog().filter((l) => l.ev === "permission");
  assert.equal(perm.length, 1);
  assert.equal(perm[0].intentSeenAtAnswer, true, "the agent saw the answer only after the intent was journaled");
  const rec = box.tools();
  assert.deepEqual(rec.map((x) => x.kind), ["intent", "outcome"]);
  const [intent, outcome] = rec;
  assert.equal(intent.toolCallId, "call-1");
  assert.equal(intent.epoch, 1);
  assert.match(intent.digest, /^[0-9a-f]{32}$/);
  assert.equal(intent.decision, "approve");
  assert.ok(intent.seq < outcome.seq);
  assert.equal(outcome.status, "completed");
  assert.equal(outcome.summary, "ok call-1");
  assert.equal(outcome.unguarded, undefined);
});

t("guard: the harness is not switched to full access and is approved with allow-once, so it keeps asking", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [tool("call-1")] }] });
  box.config({ modeKinds: ["full_access"] });
  assert.equal((await box.go({ epoch: 1, resume: false })).code, 0);
  assert.ok(!box.agentLog().some((l) => l.ev === "set_mode"));
  assert.equal(box.agentLog().find((l) => l.ev === "permission").outcome.optionId, "allow");
});

t("guard: a harness whose default mode does not ask is put in its ask-for-approval mode (guardModeKinds)", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [tool("call-1")] }] });
  box.config({ modeKinds: ["full_access"], guardModeKinds: ["standard"] });
  assert.equal((await box.go({ epoch: 1, resume: false })).code, 0);
  assert.equal(box.agentLog().find((l) => l.ev === "set_mode")?.modeId, "ask");
});

t("guard: a resume that returns no modes gets the guard mode again (the mode falls back to its default on resume)", async (box) => {
  box.script({
    prompts: [
      { match: "recovery", steps: [{ say: "ok" }] },
      { match: ".", steps: [tool("sh-1", { killRunnerBeforeOutcome: true })] },
    ],
  });
  box.config({ modeKinds: ["full_access"], guardModeKinds: ["standard"] });
  assert.equal((await box.go({ epoch: 1, resume: false })).signal, "SIGKILL");
  assert.equal((await box.go({ epoch: 2 })).code, 0);
  const sets = box.agentLog().filter((l) => l.ev === "set_mode");
  assert.deepEqual(sets.map((l) => l.modeId), ["ask", "ask"]);
  assert.ok(box.events().some((e) => e.type === "mode" && e.resumed && e.modeId === "ask"));
});

t("a generic title (\"Run command\") gets the command from the raw input, so the recovery note says which command", async (box) => {
  box.script({
    prompts: [
      { match: "recovery", steps: [{ say: "ok" }] },
      { match: ".", steps: [tool("done-1", { title: "Run command" }), tool("sh-1", { title: "Run command", killRunnerBeforeOutcome: true })] },
    ],
  });
  box.config();
  assert.equal((await box.go({ epoch: 1, resume: false })).signal, "SIGKILL");
  assert.equal((await box.go({ epoch: 2 })).code, 0);
  assert.match(box.prompts()[1], /Run command: done-1 \[completed\]/);
  assert.match(box.prompts()[1], /Run command: sh-1 \(id sh-1\)/);
});

t("guard off: full access mode and allow-always, as a plain run", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [tool("call-1")] }] });
  box.config({ modeKinds: ["full_access"], durable: { guard: false } });
  assert.equal((await box.go({ epoch: 1, resume: false })).code, 0);
  assert.deepEqual(box.agentLog().find((l) => l.ev === "set_mode")?.modeId, "full");
  assert.equal(box.agentLog().find((l) => l.ev === "permission").outcome.optionId, "always");
});

t("a crash after the intent, before the answer: the repeat request is not approved", async (box) => {
  // The journal a runner killed between "intent fsynced" and "answer sent"
  // leaves behind: the intent, no outcome, an open turn.
  box.script({
    prompts: [
      { match: "recovery", steps: [tool("call-1"), { say: "recovered" }] },
      { match: ".", steps: [] },
    ],
  });
  box.config();
  fs.mkdirSync(box.state, { recursive: true });
  const w = (f, recs) => fs.writeFileSync(path.join(box.state, f), recs.map((r) => JSON.stringify(r)).join("\n") + "\n");
  w("events.jsonl", [
    { seq: 1, ts: 1, turn: 0, epoch: 1, type: "driver_start" },
    { seq: 2, ts: 2, turn: 1, epoch: 1, type: "session", sessionId: "mock-seeded" },
    { seq: 3, ts: 3, turn: 1, epoch: 1, type: "turn_started", prompt: "do the thing", msgId: "initial" },
  ]);
  w("tools.jsonl", [{ seq: 1, ts: 3, epoch: 1, turn: 1, kind: "intent", toolCallId: "call-1", name: "run call-1", toolKind: "execute", digest: "d".repeat(32), decision: "approve" }]);
  fs.writeFileSync(path.join(box.state, "epoch"), "1");
  fs.writeFileSync(path.join(box.state, "state.json"), JSON.stringify({ turn: 1, sessionId: "mock-seeded", loadSession: true }));
  fs.writeFileSync(path.join(box.mock, "sessions.json"), JSON.stringify({ "mock-seeded": true }));
  const r = await box.go({ epoch: 2 });
  assert.equal(r.code, 0, r.stderr);
  assert.equal(box.effects().length, 0, "the repeated tool call was not run");
  const perm = box.agentLog().filter((l) => l.ev === "permission");
  assert.equal(perm.length, 1);
  assert.equal(perm[0].outcome.optionId, "reject");
  assert.ok(box.tools().some((x) => x.kind === "may_have_run" && x.toolCallId === "call-1" && x.epoch === 2));
  assert.match(box.prompts()[0], /may have partially run[\s\S]*run call-1 \(id call-1\)/);
});

t("a crash between tool start and outcome: the note lists it as may-have-run, completed calls as completed, no second approval", async (box) => {
  box.script({
    prompts: [
      { match: "recovery", steps: [tool("call-2"), { say: "recovered" }] },
      { match: ".", steps: [tool("call-1", { output: "wrote 3 files" }), tool("call-2", { killRunnerBeforeOutcome: true }), { say: "done" }] },
    ],
  });
  box.config();
  const r1 = await box.go({ epoch: 1, resume: false });
  assert.equal(r1.signal, "SIGKILL");
  assert.deepEqual(box.tools().map((x) => `${x.kind}:${x.toolCallId}`), ["intent:call-1", "outcome:call-1", "intent:call-2"]);
  assert.equal(box.effects().join(), "call-1,call-2");

  const r2 = await box.go({ epoch: 2 });
  assert.equal(r2.code, 0, r2.stderr);
  // native resume: the same session came back
  assert.ok(box.agentLog().some((l) => l.ev === "session_load"));
  const note = box.prompts()[1];
  assert.match(note, /Tool calls that completed[\s\S]*run call-1 \[completed\]: wrote 3 files/);
  assert.match(note, /may have partially run[\s\S]*run call-2 \(id call-2\)/);
  assert.match(note, /> do the thing/);
  // call-2 was approved once (epoch 1) and its repeat refused (epoch 2)
  assert.equal(box.effects().filter((e) => e === "call-2").length, 1);
  const refusals = box.agentLog().filter((l) => l.ev === "permission" && l.id === "call-2");
  assert.deepEqual(refusals.map((l) => l.outcome.optionId), ["allow", "reject"]);
  // the refusal's "failed" update is not call-2's result
  assert.equal(box.tools().filter((x) => x.kind === "outcome" && x.toolCallId === "call-2").length, 0);
});

t("a turn that already ended is never replayed", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [tool("call-1"), { say: "done" }] }] });
  box.config();
  assert.equal((await box.go({ epoch: 1, resume: false })).code, 0);
  const before = box.events().length;
  const r = await box.go({ epoch: 2 });
  assert.equal(r.code, 0, r.stderr);
  assert.equal(box.prompts().length, 1, "no second prompt");
  assert.equal(box.agentLog().filter((l) => l.ev === "start").length, 1, "the agent was not even started");
  assert.equal(box.events().filter((e) => e.type === "turn_started").length, 1);
  assert.ok(box.events().length > before); // driver_start + run_exited only
});

t("a prompt that never started is sent; with a missing journal the run starts fresh", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [{ say: "ok" }] }] });
  box.config();
  const r = await box.go({ epoch: 4, resume: true });
  assert.equal(r.code, 0, r.stderr);
  assert.deepEqual(box.prompts(), ["do the thing"]);
  assert.ok(box.events().some((e) => e.type === "notice" && /no journal/.test(e.message)));
});

t("built-in tools that ask no permission are recorded as unguarded", async (box) => {
  box.script({
    prompts: [
      { match: "recovery", steps: [{ say: "ok" }] },
      { match: ".", steps: [tool("read-1", { permission: false, kind: "read", output: "file text" }), tool("sh-1", { permission: false, killRunnerBeforeOutcome: true })] },
    ],
  });
  box.config();
  assert.equal((await box.go({ epoch: 1, resume: false })).signal, "SIGKILL");
  const rec = box.tools();
  assert.deepEqual(rec.map((x) => `${x.kind}:${x.toolCallId}:${x.unguarded ?? ""}`), ["started:read-1:true", "outcome:read-1:true", "started:sh-1:true"]);
  assert.equal((await box.go({ epoch: 2 })).code, 0);
  assert.match(box.prompts()[1], /run read-1 \[completed\]: file text/);
  assert.match(box.prompts()[1], /may have partially run[\s\S]*run sh-1 \(id sh-1\)/);
});

t("policy rerun-safe re-approves a read-only repeat but not a command", async (box) => {
  box.script({
    prompts: [
      { match: "recovery", steps: [tool("r1", { kind: "read" }), tool("x1", { kind: "execute" })] },
      { match: ".", steps: [tool("r1", { kind: "read", killRunnerBeforeOutcome: true })] },
    ],
  });
  box.config({ durable: { policy: "rerun-safe" } });
  assert.equal((await box.go({ epoch: 1, resume: false })).signal, "SIGKILL");
  // seed an execute call with an unknown outcome too
  fs.appendFileSync(path.join(box.state, "tools.jsonl"), JSON.stringify({ seq: 99, ts: 1, epoch: 1, turn: 1, kind: "intent", toolCallId: "x1", name: "run x1", toolKind: "execute", digest: "e".repeat(32), decision: "approve" }) + "\n");
  assert.equal((await box.go({ epoch: 2 })).code, 0);
  const p = box.agentLog().filter((l) => l.ev === "permission");
  assert.deepEqual(p.map((l) => `${l.id}:${l.outcome.optionId}`), ["r1:allow", "r1:allow", "x1:reject"]);
  assert.ok(box.tools().some((x) => x.kind === "rerun" && x.toolCallId === "r1"));
});

// ---- the agent crashing ------------------------------------------------------

t("an agent crash restarts the agent in-process and resumes with a recovery note", async (box) => {
  box.script({
    prompts: [
      { match: "recovery", steps: [{ say: "recovered" }] },
      { match: ".", steps: [tool("call-1"), { crash: true, once: "c1" }, { say: "unreachable" }] },
    ],
  });
  box.config();
  const r = await box.go({ epoch: 1, resume: false });
  assert.equal(r.code, 0, r.stderr);
  assert.equal(box.agentLog().filter((l) => l.ev === "start").length, 2, "one restart");
  assert.ok(box.events().some((e) => e.type === "agent_restart" && e.n === 1));
  const turns = box.events().filter((e) => e.type === "turn_started");
  assert.equal(turns.length, 2);
  assert.equal(turns[1].recovery, true);
  assert.equal(turns[1].resumesTurn, 1);
  assert.equal(turns[1].originalPrompt, "do the thing");
  assert.equal(box.events().filter((e) => e.type === "turn_ended").length, 1, "the interrupted turn is closed by the recovery turn, not by an error");
  assert.equal(box.prompts().length, 2);
});

t("an agent that keeps crashing ends the runner non-zero after maxAgentRestarts", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [{ crash: true, code: 3 }] }] });
  box.config({ durable: { maxAgentRestarts: 2 } });
  const r = await box.go({ epoch: 1, resume: false });
  assert.equal(r.code, 1);
  assert.equal(box.agentLog().filter((l) => l.ev === "start").length, 3);
  assert.equal(box.events().filter((e) => e.type === "agent_restart").length, 2);
  assert.equal(box.events().filter((e) => e.type === "turn_ended").length, 0, "the turn stays open for the re-driver");
});

// ---- no session reload -------------------------------------------------------

t("an agent without loadSession gets a new session and a degraded recovery note", async (box) => {
  box.script({
    loadSession: false,
    prompts: [
      { match: "recovery", steps: [{ say: "ok" }] },
      { match: ".", steps: [{ say: "I am halfway through the migration." }, tool("call-1", { output: "created table" }), { crash: true, once: "c1" }] },
    ],
  });
  box.config({ durable: { maxAgentRestarts: 0 } });
  const r1 = await box.go({ epoch: 1, resume: false });
  assert.equal(r1.code, 1);
  const r2 = await box.go({ epoch: 2 });
  assert.equal(r2.code, 0, r2.stderr);
  assert.equal(box.agentLog().filter((l) => l.ev === "session_new").length, 2);
  assert.ok(!box.agentLog().some((l) => l.ev === "session_load"));
  assert.ok(box.events().some((e) => e.type === "degraded"));
  const note = box.prompts()[1];
  assert.match(note, /conversation history could not be restored/);
  assert.match(note, /> do the thing/);
  assert.match(note, /halfway through the migration/);
  assert.match(note, /run call-1 \[completed\]: created table/);
  assert.equal(JSON.parse(fs.readFileSync(path.join(box.state, "state.json"), "utf8")).degraded, true);
});

// ---- exit codes ----------------------------------------------------------------

t("exit 78 when the agent cannot be started", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [] }] });
  box.config({ agent: { command: "/nonexistent/agent", args: [] } });
  assert.equal((await box.go({ epoch: 1, resume: false })).code, 78);
});

t("exit 78 for an authentication error from the agent (no restart loop)", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [] }], promptError: { message: "Authentication required", code: -32000 } });
  box.config();
  const r = await box.go({ epoch: 1, resume: false });
  assert.equal(r.code, 78);
  assert.equal(box.agentLog().filter((l) => l.ev === "start").length, 1);
});

t("exit 78 for a missing or invalid epoch", async (box) => {
  box.script({ prompts: [] });
  box.config();
  assert.equal((await box.go({ epoch: "zero" })).code, 78);
});

t("an agent with session/resume is resumed with it, and secrets never reach the journal", async (box) => {
  box.script({
    resume: true,
    loadSession: false,
    prompts: [
      { match: "recovery", steps: [{ say: "ok" }] },
      { match: ".", steps: [tool("call-1", { output: "token sk-live-0123456789abcdef used" }), { crash: true, once: "c1" }] },
    ],
  });
  box.config({ durable: { maxAgentRestarts: 0 }, secretEnv: ["MOCK_SECRET"] });
  const env = { MOCK_SECRET: "sk-live-0123456789abcdef" };
  assert.equal((await box.go({ epoch: 1, resume: false, env })).code, 1);
  assert.equal((await box.go({ epoch: 2, env })).code, 0);
  assert.ok(box.agentLog().some((l) => l.ev === "session_resume"));
  assert.ok(!box.agentLog().some((l) => l.ev === "session_new" && l.n === 2));
  for (const f of ["events.jsonl", "tools.jsonl", "state.json"]) {
    assert.ok(!fs.readFileSync(path.join(box.state, f), "utf8").includes("sk-live-0123456789abcdef"), `${f} has no secret`);
  }
  assert.match(box.tools().find((x) => x.kind === "outcome").summary, /token \[redacted\] used/);
});

// ---- the recovery note, exactly -----------------------------------------------------

t("the recovery note text", async (box) => {
  box.script({
    prompts: [
      { match: "recovery", steps: [] },
      {
        match: ".",
        steps: [tool("call-1", { title: "ls -la", output: "total 8" }), tool("call-2", { title: "git push", killRunnerBeforeOutcome: true })],
      },
    ],
  });
  box.config({ prompt: "Ship the release" });
  await box.go({ epoch: 1, resume: false });
  await box.go({ epoch: 2 });
  assert.equal(
    box.prompts()[1],
    [
      "[cua durable recovery] Your run was interrupted and has been resumed (epoch 2, previous epoch 1). This note is automatic; it is not from the user.",
      "",
      "The request you were working on (turn 1) was:",
      "> Ship the release",
      "",
      "Tool calls that completed (do not repeat them):",
      "- ls -la: call-1 [completed]: total 8",
      "",
      "Tool calls that may have partially run (started or approved, but no result was recorded). Verify the current state before repeating any of them:",
      "- git push: call-2 (id call-2)",
      "",
      "Continue the request from where it stopped.",
    ].join("\n"),
  );
});

// ---- the heartbeat ----------------------------------------------------------

t("CUA_SESSION_HEARTBEAT_SECS makes the runner print heartbeat lines on stdout; unset or 0, none", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [{ say: "hi" }, { pause: 2500 }] }] });
  box.config();
  const r = await box.go({ epoch: 4, env: { CUA_SESSION_HEARTBEAT_SECS: "1" } });
  const beats = r.stdout.split("\n").filter((l) => l.startsWith("CUA-HEARTBEAT"));
  assert.ok(beats.length >= 2, `no timer heartbeat on stdout: ${JSON.stringify(r.stdout)} ${r.stderr}`);
  assert.match(beats[0], /^CUA-HEARTBEAT 4 \d{13}$/);
  const quiet = await new Box(sdk);
  try {
    quiet.script({ prompts: [{ match: ".", steps: [{ say: "hi" }] }] });
    quiet.config();
    const q = await quiet.go({ epoch: 4, env: { CUA_SESSION_HEARTBEAT_SECS: "0" } });
    assert.ok(!q.stdout.includes("CUA-HEARTBEAT"));
  } finally {
    quiet.cleanup();
  }
});

// ---- drift guards -------------------------------------------------------------------

test("drift guard: the env contract and the ledger records match the design", () => {
  const src = fs.readFileSync(path.join(import.meta.dirname, "..", "acp-runner.mjs"), "utf8");
  // DESIGN.md "Driver launch contract" (and cua-spaces-cluster ENV_CONTRACT)
  for (const name of ["CUA_SESSION", "CUA_SESSION_EPOCH", "CUA_SESSION_ATTEMPT", "CUA_SESSION_RESUME", "CUA_SESSION_SPACE", "CUA_SESSION_STATE_DIR", "CUA_SESSION_HEARTBEAT_SECS"]) {
    assert.ok(src.includes(name), `the runner reads ${name}`);
  }
  assert.match(src, /const EXIT_PERMANENT = 78/);
  assert.match(src, /const EXIT_FENCED = 75/);
  // every ledger record kind written is documented in the header, and back
  const written = new Set([...src.matchAll(/ledger\("([a-z_]+)"/g)].map((m) => m[1]));
  const documented = new Set([...src.matchAll(/^\/\/   ([a-z_]+) +\{/gm)].map((m) => m[1]));
  assert.deepEqual([...written].sort(), [...documented].sort());
  assert.deepEqual([...written].sort(), ["duplicate", "intent", "may_have_run", "outcome", "rerun", "started"]);
});
