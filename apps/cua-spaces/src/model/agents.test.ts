// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import {
  agentInitial,
  agentName,
  agentSubtitle,
  AGENT_STATUS_DOT,
  AGENT_STATUS_LABEL,
  filterAgentRuns,
  orderAgentRuns,
  type AgentStatus,
  type SpaceAgentRun,
} from "./agents";

const ALL_STATUSES: AgentStatus[] = ["running", "idle", "failed", "crashed", "unknown"];

function run(over: Partial<SpaceAgentRun> = {}): SpaceAgentRun {
  return {
    runId: "run-1",
    agent: "claude-code",
    status: "idle",
    reason: "the last turn finished cleanly",
    summary: "build the parser",
    createdAt: 10,
    phase: "waiting",
    turn: 1,
    ...over,
  };
}

describe("agent status vocabulary", () => {
  it("labels and dots every status the shell can send", () => {
    for (const status of ALL_STATUSES) {
      expect(AGENT_STATUS_LABEL[status], status).toBeTruthy();
      expect(AGENT_STATUS_DOT[status], status).toMatch(/^status-/);
    }
  });

  it("never dresses a failure or an unknown as a healthy dot", () => {
    // The green dot means "fine". A crashed agent wearing it is the exact
    // failure this panel exists to avoid.
    expect(AGENT_STATUS_DOT.crashed).not.toBe(AGENT_STATUS_DOT.idle);
    expect(AGENT_STATUS_DOT.failed).not.toBe(AGENT_STATUS_DOT.idle);
    expect(AGENT_STATUS_DOT.unknown).not.toBe(AGENT_STATUS_DOT.idle);
    expect(AGENT_STATUS_DOT.unknown).not.toBe(AGENT_STATUS_DOT.running);
  });

  it("calls unknown unknown rather than guessing a word for it", () => {
    expect(AGENT_STATUS_LABEL.unknown).toBe("Unknown");
  });
});

describe("agentName", () => {
  it("names the wired harnesses", () => {
    expect(agentName("claude-code")).toBe("Claude Code");
    expect(agentName("openai-codex")).toBe("OpenAI Codex");
  });

  it("shows an unrecognised id rather than a blank", () => {
    expect(agentName("brand-new-cli")).toBe("brand-new-cli");
  });

  it("says so when the agent could not be read", () => {
    expect(agentName("")).toBe("Unknown agent");
    expect(agentInitial("")).toBe("U");
  });
});

describe("agentSubtitle", () => {
  it("is the prompt summary when there is one", () => {
    expect(agentSubtitle(run({ summary: "build the parser" }))).toBe("build the parser");
  });

  it("names the absence rather than rendering an empty line", () => {
    expect(agentSubtitle(run({ summary: "" }))).toContain("no prompt recorded");
    expect(agentSubtitle(run({ summary: "", agent: "" }))).toContain("could not be read");
  });
});

describe("filterAgentRuns", () => {
  const runs = [
    run({ runId: "run-a", agent: "claude-code", summary: "build the parser" }),
    run({ runId: "run-b", agent: "openai-codex", summary: "write the docs", status: "failed" }),
  ];

  it("passes everything through with no query", () => {
    expect(filterAgentRuns(runs, "  ")).toHaveLength(2);
  });

  it("matches the prompt summary, the agent name, the run id and the status", () => {
    expect(filterAgentRuns(runs, "parser").map((r) => r.runId)).toEqual(["run-a"]);
    expect(filterAgentRuns(runs, "codex").map((r) => r.runId)).toEqual(["run-b"]);
    expect(filterAgentRuns(runs, "run-a").map((r) => r.runId)).toEqual(["run-a"]);
    expect(filterAgentRuns(runs, "failed").map((r) => r.runId)).toEqual(["run-b"]);
  });

  it("is case-insensitive", () => {
    expect(filterAgentRuns(runs, "PARSER")).toHaveLength(1);
  });
});

describe("orderAgentRuns", () => {
  it("puts what needs a human above what does not", () => {
    const ordered = orderAgentRuns([
      run({ runId: "lost", status: "unknown", createdAt: 99 }),
      run({ runId: "busy", status: "running", createdAt: 50 }),
      run({ runId: "broken", status: "failed", createdAt: 1 }),
      run({ runId: "waiting", status: "idle", createdAt: 2 }),
    ]).map((r) => r.runId);
    expect(ordered).toEqual(["broken", "waiting", "busy", "lost"]);
  });

  it("is newest-first within one status", () => {
    const ordered = orderAgentRuns([
      run({ runId: "old", status: "idle", createdAt: 1 }),
      run({ runId: "new", status: "idle", createdAt: 9 }),
    ]).map((r) => r.runId);
    expect(ordered).toEqual(["new", "old"]);
  });

  it("does not drop a run with no timestamp", () => {
    const ordered = orderAgentRuns([
      run({ runId: "dated", status: "idle", createdAt: 5 }),
      run({ runId: "undated", status: "idle", createdAt: null }),
    ]);
    expect(ordered).toHaveLength(2);
    expect(ordered.map((r) => r.runId)).toContain("undated");
  });

  it("does not mutate its input", () => {
    const input = [run({ runId: "a", status: "idle" }), run({ runId: "b", status: "failed" })];
    orderAgentRuns(input);
    expect(input.map((r) => r.runId)).toEqual(["a", "b"]);
  });
});
