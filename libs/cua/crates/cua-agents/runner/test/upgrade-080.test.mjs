// A durable run's journal written by the cua 0.8.0 runner is re-driven by
// this runner (CUA-2155): `fixtures/durable-0.8.0/` is what the 0.8.0 runner
// (tag cua-sdk-v0.8.0) left when it was killed between a tool call's start
// and its outcome (its README says how). The new runner, at the next epoch,
// raises the fence, resumes the same ACP session, refuses the call whose
// outcome is unknown, says so in the recovery note, and appends to the old
// journal without rewriting a line of it. With and without journal shipping
// (durable.ship, new in this version).
import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { Box, here, sdkRoot } from "./helpers.mjs";

const sdk = sdkRoot();
const FIXTURE = path.join(here, "fixtures", "durable-0.8.0");
const tool = (id, extra = {}) => ({ tool: { id, title: `run ${id}`, kind: "execute", input: { cmd: id }, ...extra } });

function seeded() {
  const box = new Box(sdk);
  fs.mkdirSync(box.state, { recursive: true });
  for (const f of fs.readdirSync(path.join(FIXTURE, "state"))) {
    fs.copyFileSync(path.join(FIXTURE, "state", f), path.join(box.state, f));
  }
  for (const f of ["sessions.json", "effects.log"]) {
    fs.copyFileSync(path.join(FIXTURE, "mock", f), path.join(box.mock, f));
  }
  box.script({
    prompts: [
      { match: "recovery", steps: [tool("call-2"), { say: "recovered" }] },
      { match: ".", steps: [] },
    ],
  });
  return box;
}

const read = (box, f) => fs.readFileSync(path.join(box.state, f), "utf8");

async function redrive(box, extra = {}) {
  const oldEvents = read(box, "events.jsonl");
  const oldTools = read(box, "tools.jsonl");
  box.config(extra);
  if (extra.durable?.ship) {
    // A host that ships: acknowledge every tools.jsonl byte as it lands.
    const ack = setInterval(() => {
      try {
        const end = fs.statSync(path.join(box.state, "tools.jsonl")).size;
        fs.writeFileSync(path.join(box.state, "ship.ack"), JSON.stringify({ epoch: 2, end: { "tools.jsonl": end } }));
      } catch {}
    }, 20);
    try {
      return { r: await box.go({ epoch: 2 }), oldEvents, oldTools };
    } finally {
      clearInterval(ack);
    }
  }
  return { r: await box.go({ epoch: 2 }), oldEvents, oldTools };
}

for (const [label, extra] of [
  ["as 0.8.0 configured it", {}],
  ["with journal shipping", { durable: { ship: { ackFile: "ship.ack", ackTimeoutMs: 5000 } } }],
]) {
  test(`a 0.8.0 journal killed mid-tool is re-driven (${label})`, { skip: sdk ? false : "@agentclientprotocol/sdk is not available", timeout: 60_000 }, async () => {
    const box = seeded();
    try {
      const { r, oldEvents, oldTools } = await redrive(box, extra);
      assert.equal(r.code, 0, r.stderr);
      // The fence moved up; the 0.8.0 lines are untouched, new ones follow.
      assert.equal(read(box, "epoch").trim(), "2");
      assert.ok(read(box, "events.jsonl").startsWith(oldEvents), "events.jsonl was rewritten");
      assert.ok(read(box, "tools.jsonl").startsWith(oldTools), "tools.jsonl was rewritten");
      for (const e of box.events()) assert.ok(Number.isInteger(e.seq) && typeof e.type === "string", JSON.stringify(e));
      // The same ACP session came back.
      assert.ok(box.agentLog().some((l) => l.ev === "session_load"));
      // The recovery note: call-1 completed, call-2 may have run, the task.
      const note = box.prompts()[0];
      assert.match(note, /Tool calls that completed[\s\S]*run call-1 \[completed\]: wrote 3 files/);
      assert.match(note, /may have partially run[\s\S]*run call-2 \(id call-2\)/);
      assert.match(note, /> do the thing/);
      // call-2 ran once (under 0.8.0); its repeat is refused, not run again.
      assert.deepEqual(box.effects(), ["call-1", "call-2"]);
      const perm = box.agentLog().filter((l) => l.ev === "permission" && l.id === "call-2");
      assert.deepEqual(perm.map((l) => l.outcome.optionId), ["reject"]);
      assert.ok(box.tools().some((x) => x.kind === "may_have_run" && x.toolCallId === "call-2" && x.epoch === 2));
    } finally {
      box.cleanup();
    }
  });
}
