// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { beforeEach, describe, expect, it } from "vitest";

import { flagOn, setFlagOverride } from "@/bridge";

import { addAskTask, askTasks, mostUsedAgent, orderTasks, resetAskTasks, STUCK_MS, taskState, taskTitle, updateAskTask, type AskTask } from "./ask-first";

beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
});

describe("the spaces_ask_first_home flag", () => {
  it("is off unless this device turns it on", () => {
    expect(flagOn("spaces_ask_first_home")).toBe(false);
    setFlagOverride("spaces_ask_first_home", true);
    expect(flagOn("spaces_ask_first_home")).toBe(true);
    setFlagOverride("spaces_ask_first_home", null);
    expect(flagOn("spaces_ask_first_home")).toBe(false);
  });
});

describe("Ask-first tasks", () => {
  it("are kept on this device, newest first, and survive a reload", () => {
    const a = addAskTask({ prompt: "Run the tests", agent: "claude-code", os: "linux" }, 1);
    addAskTask({ prompt: "Check Safari", agent: "openai-codex", os: "macos" }, 2);
    updateAskTask(a.id, { runId: "run-1", spaceId: "local:x" });
    resetAskTasks();
    expect(askTasks().map((t) => t.prompt)).toEqual(["Check Safari", "Run the tests"]);
    expect(askTasks()[1]).toMatchObject({ runId: "run-1", spaceId: "local:x" });
  });

  it("read as starting, working, needs you, stuck, done and thrown away", () => {
    const t = addAskTask({ prompt: "x", agent: "claude-code", os: "linux" });
    const now = 1_000_000_000;
    expect(taskState(t, null)).toBe("starting");
    const running: AskTask = { ...t, runId: "r", spaceId: "s" };
    expect(taskState(running, { status: "running", needsYou: false, lastEventMs: now }, now)).toBe("working");
    expect(taskState(running, { status: "running", needsYou: false, lastEventMs: now - STUCK_MS - 1 }, now)).toBe("stuck");
    expect(taskState(running, { status: "idle", needsYou: true, lastEventMs: now }, now)).toBe("needs-you");
    expect(taskState(running, { status: "idle", needsYou: false, lastEventMs: now }, now)).toBe("done");
    expect(taskState({ ...running, ending: "kept" }, { status: "idle", needsYou: false, lastEventMs: now }, now)).toBe("kept");
    expect(taskState({ ...running, ending: "thrown" }, null)).toBe("thrown");
    expect(taskState({ ...t, error: "no room" }, null)).toBe("failed");
  });

  it("put what needs you first, then running ones, then the rest", () => {
    const mk = (id: string, createdMs: number) => ({ ...addAskTask({ prompt: id, agent: "claude-code", os: "linux" }), createdMs });
    const rows = orderTasks([
      { task: mk("done", 3), state: "done" as const },
      { task: mk("working", 1), state: "working" as const },
      { task: mk("asks", 2), state: "needs-you" as const },
    ]);
    expect(rows.map((r) => r.task.prompt)).toEqual(["asks", "working", "done"]);
  });

  it("pick the coding app used most, and a short title", () => {
    expect(mostUsedAgent([])).toBeNull();
    const t = (agent: string) => ({ agent }) as AskTask;
    expect(mostUsedAgent([t("openai-codex"), t("claude-code"), t("openai-codex")])).toBe("openai-codex");
    expect(taskTitle("  \nFirst line\nsecond")).toBe("First line");
    expect(taskTitle("x".repeat(100), 10)).toBe(`${"x".repeat(9)}…`);
  });
});
