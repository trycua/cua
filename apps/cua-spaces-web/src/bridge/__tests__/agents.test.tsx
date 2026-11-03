// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { HostError } from "../adapter";
import { createDemoAdapter } from "../adapters/demo";
import type { AgentEvent } from "../contracts/agents";
import type { CoreClient } from "../core";
import { BridgeProvider, useAgentTimeline, useAgents, type AgentsHook, type AgentTimeline, type RunRef } from "../index";
import { Transcript } from "../transcript";
import { noCore, testCore } from "./testCore";

afterEach(cleanup);

/* ---- The transcript fold (cua-agents transcript.rs) ------------------------- */

let seq = 0;
const ev = (e: Partial<AgentEvent> & Pick<AgentEvent, "kind" | "category">): AgentEvent => ({ seq: ++seq, ts_ms: seq, turn: 1, ...e });

describe("transcript", () => {
  it("joins message chunks, keeps one prompt per turn, folds activity and drops hidden events", () => {
    seq = 0;
    const t = new Transcript();
    t.absorbAll([
      ev({ kind: "turn_started", category: "user", text: "fix the build" }),
      ev({ kind: "user_message", category: "user", text: "fix the build" }),
      ev({ kind: "thought", category: "activity", text: "look at CI\nthen fix" }),
      ev({ kind: "tool_call", category: "activity", tool_id: "t1", tool_title: "cargo build" }),
      ev({ kind: "tool_update", category: "activity", tool_id: "t1", tool_status: "failed", text: "error[E0308]" }),
      ev({ kind: "usage", category: "hidden" }),
      ev({ kind: "message", category: "message", text: "It was " }),
      ev({ kind: "message", category: "message", text: "a type error." }),
      ev({ kind: "error", category: "activity", text: "runner exited" }),
    ]);
    expect(t.items.map((i) => [i.kind, i.text])).toEqual([
      ["user", "fix the build"],
      ["activity", "2 steps"],
      ["message", "It was a type error."],
      ["activity", "1 step, 1 error"],
    ]);
    expect(t.items[1]!.steps.map((s) => [s.text, s.failed])).toEqual([
      ["Thinking: look at CI", false],
      ["Tool cargo build failed: error[E0308]", true],
    ]);
  });

  it("skips events it already absorbed and replaces only the items it touched", () => {
    seq = 0;
    const t = new Transcript();
    const first = [ev({ kind: "turn_started", category: "user", text: "hi" }), ev({ kind: "message", category: "message", text: "Hel" })];
    t.absorbAll(first);
    const before = t.items;
    expect(t.absorbAll(first)).toBe(false);
    expect(t.items).toBe(before);
    t.absorbAll([ev({ kind: "message", category: "message", text: "lo" })]);
    expect(t.items).not.toBe(before);
    expect(t.items[0]).toBe(before[0]);
    expect(t.items[1]!.text).toBe("Hello");
    expect(t.items[1]!.id).toBe(before[1]!.id);
  });
});

/* ---- The demo host --------------------------------------------------------- */

function clockedDemo() {
  const clock = { t: Date.UTC(2026, 9, 3, 12) };
  const adapter = createDemoAdapter({ latencyMs: 0, stepMs: 2, now: () => clock.t });
  return { adapter, clock };
}

describe("demo agents", () => {
  it("lists four persistent agents across Claude Code, Codex, Hermes and OpenClaw", async () => {
    const { adapter } = clockedDemo();
    const agents = await adapter.call("agents.list", {});
    expect(agents.map((a) => a.harness).sort()).toEqual(["claude-code", "hermes", "openai-codex", "openclaw"]);
    adapter.dispose?.();
  });

  it("refuses runs in a Space that is off", async () => {
    const { adapter } = clockedDemo();
    await expect(adapter.call("agents.runs", { spaceId: "local:agent-sandbox" })).rejects.toMatchObject({ code: "space_off" });
    expect(await adapter.call("agents.runs", { spaceId: "local:design-review" })).toHaveLength(2);
    adapter.dispose?.();
  });

  it("writes the live run's events over time and pages them by cursor", async () => {
    const { adapter, clock } = clockedDemo();
    const ref = { spaceId: "local:design-review", runId: "run-ada-7" };
    const first = await adapter.call("agents.events", { ...ref, cursor: 0, max: 5 });
    expect(first.events.map((e) => e.seq)).toEqual([1, 2, 3, 4, 5]);
    expect(first.caught_up).toBe(false);
    let page = await adapter.call("agents.events", { ...ref, cursor: first.cursor });
    expect(page.status).toBe("running");
    expect(page.caught_up).toBe(true);
    const written = page.cursor;
    clock.t += 60_000;
    page = await adapter.call("agents.events", { ...ref, cursor: written });
    expect(page.events.length).toBeGreaterThan(10);
    expect(page.events[page.events.length - 1]!.kind).toBe("turn_ended");
    expect(page.status).toBe("idle");
    adapter.dispose?.();
  });

  it("pausing an agent stops its run where it is", async () => {
    const { adapter, clock } = clockedDemo();
    const ref = { spaceId: "local:design-review", runId: "run-ada-7" };
    await adapter.call("agents.pause", { name: "ada" });
    const ada = (await adapter.call("agents.list", {})).find((a) => a.name === "ada")!;
    expect(ada).toMatchObject({ paused: true, runId: null, spaceState: "suspended" });
    clock.t += 60_000;
    const page = await adapter.call("agents.events", { ...ref, cursor: 0, max: 500 });
    expect(page.events[page.events.length - 1]).toMatchObject({ kind: "turn_ended", stop_reason: "cancelled" });
    expect(page.status).toBe("idle");
    adapter.dispose?.();
  });

  it("sets up every installed agent", async () => {
    const { adapter } = clockedDemo();
    const rows = await adapter.call("agents.configure", { agents: null });
    expect(rows.filter((r) => r.installed).every((r) => r.configured)).toBe(true);
    expect(rows.find((r) => !r.installed)?.configured).toBe(false);
    adapter.dispose?.();
  });
});

/* ---- Hooks over the demo ------------------------------------------------------ */

function mount(core: CoreClient, run: RunRef | null) {
  const { adapter, clock } = clockedDemo();
  const got: { agents?: AgentsHook; timeline?: AgentTimeline } = {};
  function Probe() {
    got.agents = useAgents();
    got.timeline = useAgentTimeline(run);
    return null;
  }
  render(
    <BridgeProvider adapter={adapter} core={core} storeOptions={{ eventsPollMs: { live: 5, idle: 5 } }}>
      <Probe />
    </BridgeProvider>,
  );
  return { got, clock };
}

describe.each([
  ["with the core", testCore],
  ["without the core", async () => noCore],
])("useAgents %s", (_label, makeCore) => {
  it("joins persistent agents with the runs in running Spaces", async () => {
    const { got } = mount(await makeCore(), null);
    await waitFor(() => expect(got.agents?.data).toBeDefined());
    const data = got.agents!.data!;
    expect(data.agents.map((a) => [a.name, a.state])).toEqual([
      ["ada", "running"],
      ["atlas", "idle"],
      ["claw", "idle"],
      ["scout", "paused"],
    ]);
    expect(data.agents[0]).toMatchObject({ harnessName: "Claude Code", run: { spaceId: "local:design-review", runId: "run-ada-7" } });
    // atlas has no current run: it opens its latest one in its Space.
    expect(data.agents[1]!.run).toEqual({ spaceId: "relay:linux-box/ubuntu-build", runId: "run-atlas-12" });
    expect(data.runs).toHaveLength(5);
    expect(data.runs[0]).toMatchObject({ runId: "run-9b20", status: "failed" });
    expect(data.runs.find((r) => r.runId === "run-ada-7")?.agentName).toBe("ada");
  });

  it("pauses and resumes through the host", async () => {
    const { got } = mount(await makeCore(), null);
    await waitFor(() => expect(got.agents?.data).toBeDefined());
    await act(() => got.agents!.pauseAgent("atlas"));
    await waitFor(() => expect(got.agents!.data!.agents.find((a) => a.name === "atlas")?.state).toBe("paused"));
    expect(got.agents!.data!.agents.find((a) => a.name === "atlas")?.actionLabel).toBe("Resume");
    await act(() => got.agents!.resumeAgent("atlas"));
    await waitFor(() => expect(got.agents!.data!.agents.find((a) => a.name === "atlas")?.state).toBe("idle"));
  });
});

describe("useAgentTimeline", () => {
  it("streams a running run's conversation and follows it to the end", async () => {
    const { got, clock } = mount(noCore, { spaceId: "local:design-review", runId: "run-ada-7" });
    await waitFor(() => expect(got.timeline?.items.length).toBeGreaterThan(0));
    expect(got.timeline!.status).toBe("running");
    const before = got.timeline!.items.length;
    const firstItem = got.timeline!.items[0];
    act(() => {
      clock.t += 60_000;
    });
    await waitFor(() => expect(got.timeline!.status).toBe("idle"));
    expect(got.timeline!.items.length).toBeGreaterThan(before);
    expect(got.timeline!.items[0]).toBe(firstItem);
    const last = got.timeline!.items.filter((i) => i.kind === "message").pop()!;
    expect(last.text).toContain("## Onboarding copy");
    expect(last.text).toContain("```diff");
    // The run went idle, so the list reads it again.
    await waitFor(() => expect(got.agents?.data?.runs.find((r) => r.runId === "run-ada-7")?.status).toBe("idle"));
  });

  it("reports a host that cannot read events", async () => {
    const { adapter } = clockedDemo();
    const unsupported: typeof adapter = {
      ...adapter,
      call: (op, args) => (op === "agents.events" ? Promise.reject(new HostError("agents.events is not available", "unsupported")) : adapter.call(op, args)),
    };
    let timeline: AgentTimeline | undefined;
    function Probe() {
      timeline = useAgentTimeline({ spaceId: "local:design-review", runId: "run-ada-7" });
      return null;
    }
    render(
      <BridgeProvider adapter={unsupported} core={noCore}>
        <Probe />
      </BridgeProvider>,
    );
    await waitFor(() => expect(timeline?.unsupported).toBe(true));
    expect(timeline!.isLoading).toBe(false);
  });
});
