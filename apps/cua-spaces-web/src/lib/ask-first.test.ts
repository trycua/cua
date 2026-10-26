// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { beforeEach, describe, expect, it } from "vitest";

import { flagOn, setFlagOverride } from "@/bridge";

import type { Space } from "@/bridge";

import {
  addAskTask,
  askTasks,
  currentWhere,
  defaultWhere,
  HOST_NOT_SHARED,
  hostSpaceId,
  setThisMacConfirmed,
  thisMacConfirmed,
  thisMacConfirmText,
  mostUsedAgent,
  orderTasks,
  pickableSpaces,
  resetAskTasks,
  retryTarget,
  startErrorView,
  STUCK_MS,
  taskState,
  TASK_STATE_TEXT,
  taskTarget,
  taskTitle,
  updateAskTask,
  type AskTask,
  type NewKind,
} from "./ask-first";

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
    expect(taskState({ ...t, error: "no room" }, null)).toBe("didnt-start");
    expect(taskState({ ...running, error: "lost the run" }, null)).toBe("failed");
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

const row = (id: string, more: Partial<Space> = {}): Space => ({ id, name: id, os: "linux", status: "running", detail: "", lastUsedAt: 0, scene: "linux-terminal", ...more });
const thisMac = row("this-mac", { name: "This computer", os: "macos", status: "local", provider: "local" });
const LINUX: NewKind = { os: "linux", note: "Free.", blocked: null };
const MAC: NewKind = { os: "macos", note: "Free.", blocked: null };
const shared = { configured: true, mode: "relay" as const, machineId: "m-ada-mbp", sharing: true, shareDesktop: true };

describe("where a task runs", () => {
  it("maps each pick to what agents.start is pointed at", () => {
    // A new Linux Space is made first, then the run starts in it.
    expect(taskTarget({ kind: "new", os: "linux" })).toEqual({ kind: "create", os: "linux" });
    // A local Space and a cloud Space are passed by their own ids.
    expect(taskTarget({ kind: "space", space: row("local:aurora", { provider: "local" }) })).toEqual({ kind: "space", spaceId: "local:aurora" });
    expect(taskTarget({ kind: "space", space: row("cloud:ci-box", { provider: "direct", cloud: "aws" }) })).toEqual({ kind: "space", spaceId: "cloud:ci-box" });
    // This Mac (its own row, or the picker's This Mac) starts on its host Space id.
    expect(taskTarget({ kind: "this-mac" })).toEqual({ kind: "host" });
    expect(taskTarget({ kind: "space", space: thisMac })).toEqual({ kind: "host" });
  });

  it("finds this Mac's own Space id from its host service", () => {
    expect(hostSpaceId(shared)).toBe("relay:m-ada-mbp");
    expect(hostSpaceId({ ...shared, mode: "direct", machineId: null, directUrl: "http://10.0.0.5:3211" })).toBe("direct:10.0.0.5:3211");
    // Not set up, not sharing, or not sharing its desktop: no Space id, so the start says how to fix it.
    expect(hostSpaceId(null)).toBeNull();
    expect(hostSpaceId({ ...shared, configured: false })).toBeNull();
    expect(hostSpaceId({ ...shared, sharing: false })).toBeNull();
    expect(hostSpaceId({ ...shared, shareDesktop: false })).toBeNull();
  });

  it("retries on the task's own computer, or makes a new one", () => {
    expect(retryTarget({ spaceId: null, os: "linux" })).toEqual({ kind: "create", os: "linux" });
    expect(retryTarget({ spaceId: "local:aurora", os: "linux" })).toEqual({ kind: "space", spaceId: "local:aurora" });
    expect(retryTarget({ spaceId: "relay:m-ada-mbp", os: "macos", onHost: true })).toEqual({ kind: "host" });
    // A task from before: the pseudo-id now starts on this Mac's Space id.
    expect(retryTarget({ spaceId: "this-mac", os: "macos" })).toEqual({ kind: "host" });
  });

  it("lists running Spaces; this Mac has its own row in the picker", () => {
    const spaces = [thisMac, row("local:aurora"), row("local:off", { power: { control: "stop", off: true } as Space["power"] })];
    expect(pickableSpaces(spaces).map((s) => s.id)).toEqual(["local:aurora"]);
  });

  it("defaults to a new Linux computer on this Mac", () => {
    expect(defaultWhere([MAC, LINUX])).toEqual({ kind: "new", os: "linux" });
    expect(defaultWhere([])).toEqual({ kind: "new", os: "linux" });
    expect(defaultWhere([{ ...LINUX, blocked: "Needs Docker" }, MAC])).toEqual({ kind: "new", os: "macos" });
    expect(currentWhere(null, [LINUX, MAC], [row("local:aurora")])).toEqual({ kind: "new", os: "linux" });
  });

  it("keeps a pick while it's offered, and falls back when it goes", () => {
    const aurora = row("local:aurora");
    expect(currentWhere({ kind: "space", space: aurora }, [LINUX], [aurora])).toEqual({ kind: "space", space: aurora });
    expect(currentWhere({ kind: "space", space: aurora }, [LINUX], [])).toEqual({ kind: "new", os: "linux" });
    expect(currentWhere({ kind: "this-mac" }, [LINUX], [])).toEqual({ kind: "this-mac" });
    expect(currentWhere({ kind: "new", os: "macos" }, [LINUX, MAC], [])).toEqual({ kind: "new", os: "macos" });
    expect(currentWhere({ kind: "new", os: "windows" }, [LINUX], [])).toEqual({ kind: "new", os: "linux" });
  });

  it("remembers the answer to Work on this Mac? on this device until Settings resets it", () => {
    expect(thisMacConfirmed()).toBe(false);
    setThisMacConfirmed(true);
    expect(thisMacConfirmed()).toBe(true);
    expect(localStorage.getItem("cua.askFirst.thisMacOk")).toBe("1");
    setThisMacConfirmed(false);
    expect(thisMacConfirmed()).toBe(false);
  });

  it("says plainly what working on this Mac means", () => {
    expect(thisMacConfirmText("Codex").description).toContain("Codex will work directly on this Mac, with your files and apps. It can't be undone by throwing the computer away.");
    expect(thisMacConfirmText("Codex").description).toContain("doesn't ask before each step");
  });
});

describe("a start that failed", () => {
  const CORE_NOT_FOUND = 'Space "this-mac" (use its id, such as local:<name> or cloud:<name>, or add it with add_space) not found';

  it("reads as Didn't start, the same in the header and on the screen", () => {
    expect(TASK_STATE_TEXT["didnt-start"]).toBe("Didn't start");
  });

  it("says what went wrong in plain words, with one thing to do", () => {
    expect(startErrorView(CORE_NOT_FOUND, "Codex")).toMatchObject({ title: "Couldn't start on this Mac", action: "retry", retry: true });
    expect(startErrorView(HOST_NOT_SHARED, "Codex")).toMatchObject({ title: "This Mac isn't set up for tasks yet", action: "set-up-host", retry: true });
    expect(startErrorView('Space "local:gone" (use its id, such as local:<name> or cloud:<name>, or add it with add_space) not found', "Codex")).toMatchObject({
      title: "Couldn't find that computer",
      body: "It may have been deleted or renamed. Pick another one.",
      action: "change-computer",
    });
    expect(startErrorView("Cua Cloud has ended; use a local sandbox or your own computer.", "Codex").action).toBe("change-computer");
    expect(startErrorView("OPENAI_API_KEY is not set", "Codex")).toMatchObject({ title: "Codex needs your key", action: "add-key", retry: true });
    expect(startErrorView("aurora is not running", "Codex")).toMatchObject({ title: "Couldn't reach that computer", action: "retry" });
    expect(startErrorView("Not enough disk space left", "Codex").action).toBe("retry");
    expect(startErrorView("boom", "Codex")).toMatchObject({ title: "Codex couldn't start", action: "retry", retry: true });
  });

  it("never shows the core's jargon in the headline or the line under it", () => {
    for (const raw of [CORE_NOT_FOUND, HOST_NOT_SHARED, 'Space "x" not found', "boom", "OPENAI_API_KEY is not set"]) {
      const v = startErrorView(raw, "Codex");
      expect(`${v.title} ${v.body}`).not.toMatch(/add_space|local:<|cloud:<|this-mac|API_KEY|host service/);
    }
  });
});
