// Journal shipping (durable.ship): the runner answers a permission request
// only after the host acknowledged, in `ship.ack`, that the intent is on the
// Cua Volume. The host is played by a fake shipper that delays its acks.
//
//   node --test libs/cua/crates/cua-agents/runner/test/ship.test.mjs
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

const tool = (id) => ({ tool: { id, title: `run ${id}`, kind: "execute", input: { cmd: id } } });

// A host that "ships" tools.jsonl `delayMs` after each change and then acks.
function fakeShipper(box, epoch, delayMs) {
  const acks = [];
  let shipped = 0;
  let stop = false;
  const file = path.join(box.state, "tools.jsonl");
  const loop = (async () => {
    // Bounded: the test's own timeout ends it at the latest.
    for (let i = 0; i < 2000 && !stop; i++) {
      let size = 0;
      try {
        size = fs.statSync(file).size;
      } catch {}
      if (size > shipped) {
        await new Promise((r) => setTimeout(r, delayMs));
        shipped = size;
        const at = Date.now();
        fs.writeFileSync(path.join(box.state, "ship.ack"), JSON.stringify({ epoch, end: { "tools.jsonl": size } }));
        acks.push({ at, end: size });
      }
      await new Promise((r) => setTimeout(r, 20));
    }
  })();
  return {
    acks,
    async stop() {
      stop = true;
      await loop;
    },
  };
}

t("an intent is answered only after the host acked it on the Volume", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [tool("t1"), tool("t2"), { say: "done" }] }] });
  box.config({ durable: { ship: { ackFile: "ship.ack", ackTimeoutMs: 10_000 } } });
  const shipper = fakeShipper(box, 1, 400);
  const r = await box.go({ epoch: 1, resume: false });
  await shipper.stop();
  assert.equal(r.code, 0, r.stderr);
  const answers = box.agentLog().filter((l) => l.ev === "permission");
  assert.equal(answers.length, 2);
  const intents = [];
  let off = 0;
  for (const line of fs.readFileSync(path.join(box.state, "tools.jsonl"), "utf8").split("\n").filter(Boolean)) {
    off += Buffer.byteLength(line) + 1;
    const rec = JSON.parse(line);
    if (rec.kind === "intent") intents.push({ id: rec.toolCallId, end: off });
  }
  for (const a of answers) {
    assert.equal(a.outcome.outcome, "selected");
    assert.notEqual(a.outcome.optionId, "reject");
    const intent = intents.find((i) => i.id === a.id);
    // The ack the agent could see at its answer covers the intent ...
    assert.ok(a.ackAtAnswer && a.ackAtAnswer.end["tools.jsonl"] >= intent.end, JSON.stringify(a));
    // ... and the first ack that covered it came before the answer.
    const ack = shipper.acks.find((k) => k.end >= intent.end);
    assert.ok(ack && ack.at <= a.at, `ack ${JSON.stringify(ack)} answer ${a.at}`);
  }
  assert.deepEqual(box.effects(), ["t1", "t2"]);
});

t("no ack in time: the call is refused, journaled as denied, and never runs", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [tool("t1"), { say: "done" }] }] });
  box.config({ durable: { ship: { ackFile: "ship.ack", ackTimeoutMs: 600 } } });
  const r = await box.go({ epoch: 1, resume: false });
  assert.equal(r.code, 0, r.stderr);
  const a = box.agentLog().find((l) => l.ev === "permission");
  assert.equal(a.outcome.optionId, "reject");
  assert.deepEqual(box.effects(), []);
  const recs = box.tools().filter((x) => x.toolCallId === "t1");
  assert.deepEqual(recs.map((x) => x.kind), ["intent", "outcome"]);
  assert.equal(recs[1].denied, true);
  assert.match(recs[1].summary, /did not reach the Cua Volume/);
});

t("an ack from another epoch does not count", async (box) => {
  box.script({ prompts: [{ match: ".", steps: [tool("t1"), { say: "done" }] }] });
  box.config({ durable: { ship: { ackFile: "ship.ack", ackTimeoutMs: 800 } } });
  fs.mkdirSync(box.state, { recursive: true });
  fs.writeFileSync(path.join(box.state, "ship.ack"), JSON.stringify({ epoch: 1, end: { "tools.jsonl": 1e9 } }));
  const r = await box.go({ epoch: 2, resume: false });
  assert.equal(r.code, 0, r.stderr);
  assert.deepEqual(box.effects(), []);
});
