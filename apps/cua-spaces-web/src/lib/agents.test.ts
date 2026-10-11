// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import type { AgentRunSummary, AgentsData, AgentSummary } from "@/bridge";

import { resolveSelection, shortAge, spaceLabel } from "./agents";

const agent = (name: string, state: AgentSummary["state"], run: AgentSummary["run"]): AgentSummary => ({
  name,
  harness: "claude-code",
  harnessName: "Claude Code",
  spaceId: "local:dev",
  spaceName: "Dev",
  spaceState: "running",
  state,
  detail: "",
  actionLabel: state === "paused" ? "Resume" : "Pause",
  lastActivityMs: 0,
  run,
  lastError: null,
});

const run = (runId: string, agentName: string | null = null): AgentRunSummary => ({
  runId,
  agent: "claude-code",
  status: "idle",
  reason: "",
  summary: runId,
  createdAt: 1,
  phase: "",
  turn: 1,
  spaceId: "local:dev",
  spaceName: "Dev",
  harnessName: "Claude Code",
  startedMs: 1000,
  agentName,
});

const data: AgentsData = {
  agents: [agent("ada", "idle", { spaceId: "local:dev", runId: "r1" }), agent("bo", "running", { spaceId: "local:dev", runId: "r2" })],
  runs: [run("r1", "ada"), run("r2", "bo"), run("r3")],
  unread: [],
  canList: true,
};

describe("resolveSelection", () => {
  it("opens a named agent's run", () => {
    expect(resolveSelection(data, { agent: "ada" })).toMatchObject({ agent: { name: "ada" }, run: { runId: "r1" } });
  });

  it("opens a named run, with its agent when it has one", () => {
    expect(resolveSelection(data, { space: "local:dev", run: "r3" })).toMatchObject({ agent: null, run: { runId: "r3" } });
    expect(resolveSelection(data, { space: "local:dev", run: "r2" }).agent?.name).toBe("bo");
  });

  it("falls back to a running agent, then the first agent, then the first run", () => {
    expect(resolveSelection(data, { agent: "gone" }).agent?.name).toBe("bo");
    expect(resolveSelection({ ...data, agents: [] }, {}).run?.runId).toBe("r1");
    expect(resolveSelection(undefined, {})).toEqual({ agent: null, run: null, ref: null });
  });
});

describe("labels", () => {
  it("shortens ages and Space names", () => {
    const now = 10 * 86_400_000;
    expect([shortAge(now - 5_000, now), shortAge(now - 5 * 60_000, now), shortAge(now - 3 * 3_600_000, now), shortAge(now - 2 * 86_400_000, now)]).toEqual([
      "now",
      "5m",
      "3h",
      "2d",
    ]);
    expect(shortAge(0, now)).toBe("");
    expect(spaceLabel("relay:linux-box/ubuntu-build")).toBe("ubuntu-build");
    expect(spaceLabel("local:dev")).toBe("dev");
    expect(spaceLabel("Design Review")).toBe("Design Review");
  });
});
