// A run that continues the agent's own earlier session (agent_start's
// resume_session, CUA-3319): the runner reloads it, and when the agent
// can't, the first prompt carries the earlier conversation instead.
//
//   node --test libs/cua/crates/cua-agents/runner/test/
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { Box, sdkRoot } from "./helpers.mjs";

const sdk = sdkRoot();
const t = (name, fn) => test(name, { skip: sdk ? false : "@agentclientprotocol/sdk is not available", timeout: 60_000 }, async () => {
  const box = new Box(sdk);
  try {
    await fn(box);
  } finally {
    box.cleanup();
  }
});

/** A one-off (not durable) run that resumes `sessionId`. */
function resumeRun(box, { sessionId, context, known, script = {} }) {
  box.script({ prompts: [{ match: ".*", steps: [{ say: "carrying on" }] }], ...script });
  if (known) fs.writeFileSync(path.join(box.mock, "sessions.json"), JSON.stringify({ [sessionId]: true }));
  fs.writeFileSync(
    path.join(box.run, "run.json"),
    JSON.stringify({
      agent: { command: process.execPath, args: [path.join(box.root, "mock-acp-agent.mjs")] },
      cwd: box.run,
      prompt: "now pin it to UTC",
      env: { MOCK_DIR: box.mock, MOCK_SCRIPT: path.join(box.mock, "script.json") },
      exitWhenIdle: true,
      resume: { sessionId, context },
    }),
  );
}

t("reloads the agent's own session and sends the prompt as typed", async (box) => {
  resumeRun(box, { sessionId: "old-chat-1", context: "person: the timer test fails", known: true });
  const r = await box.go({ env: { CUA_SESSION: "", CUA_SESSION_EPOCH: "" } });
  assert.equal(r.code, 0, r.stderr);
  const log = box.agentLog();
  assert.ok(log.some((l) => l.ev === "session_load" && l.id === "old-chat-1"), JSON.stringify(log));
  assert.ok(!log.some((l) => l.ev === "session_new"));
  assert.deepEqual(box.prompts(), ["now pin it to UTC"]);
});

t("uses session/resume when the agent offers it", async (box) => {
  resumeRun(box, { sessionId: "old-chat-2", known: true, script: { resume: true } });
  const r = await box.go({ env: { CUA_SESSION: "", CUA_SESSION_EPOCH: "" } });
  assert.equal(r.code, 0, r.stderr);
  assert.ok(box.agentLog().some((l) => l.ev === "session_resume" && l.id === "old-chat-2"));
  assert.deepEqual(box.prompts(), ["now pin it to UTC"]);
});

for (const resume of [false, true]) {
  t(`session/${resume ? "resume" : "load"} history is not published as a new reply or tool call`, async (box) => {
    resumeRun(box, {
      sessionId: "replayed-chat",
      known: true,
      script: {
        resume,
        replay: [
          { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "an old answer" } },
          { sessionUpdate: "tool_call", toolCallId: "past-tool", title: "a completed old tool", kind: "other", status: "completed" },
          { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "the last old answer" } },
        ],
      },
    });
    const r = await box.go({ env: { CUA_SESSION: "", CUA_SESSION_EPOCH: "" } });
    assert.equal(r.code, 0, r.stderr);
    const updates = box.lines("events.jsonl", box.run).filter((e) => e.type === "update").map((e) => e.update);
    assert.deepEqual(updates.filter((u) => u.sessionUpdate === "agent_message_chunk").map((u) => u.content.text), ["carrying on"]);
    assert.ok(!updates.some((u) => u.toolCallId === "past-tool"), "historical tool call was republished");
  });
}

t("a session the agent can't reload: a new one, with the earlier conversation first", async (box) => {
  resumeRun(box, { sessionId: "gone-chat", context: "person: the timer test fails\n\nagent: it depends on the time zone", known: false });
  const r = await box.go({ env: { CUA_SESSION: "", CUA_SESSION_EPOCH: "" } });
  assert.equal(r.code, 0, r.stderr);
  assert.ok(box.agentLog().some((l) => l.ev === "session_new"));
  const [prompt] = box.prompts();
  assert.match(prompt, /could not reload/);
  assert.match(prompt, /it depends on the time zone/);
  assert.ok(prompt.endsWith("now pin it to UTC"), prompt);
});
