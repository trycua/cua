// The steps of an agent joining a Space, as the runner writes them to the
// run's events (the host reads them for `cua_agent_join_phase` and for the
// "joined" note): runner_started, agent_spawned, initialized, session,
// turn_started, the first words, turn_ended, in that order, each stamped in
// the guest. Runs the real runner against the scripted mock agent (a stub
// harness: no model, no network) and prints the per-step timings.
//
//   node --test libs/cua/crates/cua-agents/runner/test/
import { test } from "node:test";
import assert from "node:assert/strict";
import { Box, sdkRoot } from "./helpers.mjs";

const sdk = sdkRoot();

test("a join writes its steps in order with guest timestamps", { skip: sdk ? false : "@agentclientprotocol/sdk is not available", timeout: 60_000 }, async () => {
  const box = new Box(sdk);
  try {
    box.script({ prompts: [{ match: ".", steps: [{ say: "hello" }] }] });
    box.config();
    const t0 = Date.now();
    const r = await box.go({ epoch: 1 });
    assert.equal(r.code, 0, r.stderr);
    const evs = box.events();
    const first = (type, pred = () => true) => evs.find((e) => e.type === type && pred(e));
    const steps = [
      ["runner_started", first("runner_started")],
      ["agent_spawned", first("agent_spawned")],
      ["initialized", first("initialized")],
      ["session", first("session")],
      ["turn_started", first("turn_started")],
      ["first words", first("update", (e) => e.update?.sessionUpdate === "agent_message_chunk")],
      ["turn_ended", first("turn_ended")],
    ];
    for (const [name, e] of steps) assert.ok(e, `${name} is written`);
    const at = steps.map(([, e]) => e.ts);
    for (let i = 1; i < at.length; i++) assert.ok(at[i] >= at[i - 1], `${steps[i][0]} comes after ${steps[i - 1][0]}`);
    assert.ok(at[0] >= t0 - 1000, "stamped by the guest clock");
    const timings = steps.map(([name, e], i) => `${name} +${e.ts - at[0]} ms`);
    console.log(`join steps (stub harness): ${timings.join(", ")}`);
  } finally {
    box.cleanup();
  }
});
