// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// CUA-3319: "Pick up where you left off". Continue in Cua opens the chat in a
// private Home task with its earlier messages; nothing runs until the person
// sends, and then the agent resumes its own session on this Mac, in the
// chat's folder.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { BridgeProvider, setFlagOverride, type HostStatus } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { PickUpWhereYouLeftOff, ReadyScreen } from "@/components/first-run-new/first-run-new";
import { readyView } from "@/lib/first-run-new";
import { addImportedTask, askTask, resetAskTasks, type AskTask } from "@/lib/ask-first";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
  setFlagOverride("spaces_ask_first_home", true);
});
afterEach(() => cleanup());

const q = (s: string) => document.querySelector<HTMLElement>(s);
const KETTLE = { agent: "claude-code", id: "0b1e5c2a-1111-4222-8333-944445555666", title: "Kettle timer test fails on CI", cwd: "/Users/demo/code/kettle", lastActiveMs: Date.now() - 60_000 };
const SHARED: Partial<HostStatus> = { configured: true, sharing: true, shareDesktop: true, mode: "relay", machineId: "this-mac-1" };

describe("Pick up where you left off", () => {
  it("lists the chats with their folder and time, and Continue in Cua makes a private Home task", async () => {
    const picked: AskTask[] = [];
    render(
      <BridgeProvider adapter={createDemoAdapter({ latencyMs: 1 })} core={await testCore()}>
        <PickUpWhereYouLeftOff onContinue={(t) => picked.push(t)} />
      </BridgeProvider>,
    );
    await waitFor(() => expect(document.querySelectorAll("[data-pick-up-row]").length).toBe(3));
    expect(screen.getByText("Kettle timer test fails on CI")).toBeTruthy();
    expect(screen.getByText("~/code/kettle, 25 min ago")).toBeTruthy();
    expect(screen.getByText("Your chats stay on this Mac until you pick one.")).toBeTruthy();
    fireEvent.click(document.querySelectorAll<HTMLElement>("[data-pick-up-continue]")[0]!);
    expect(picked).toHaveLength(1);
    expect(picked[0]!.imported).toMatchObject({ sessionId: KETTLE.id, cwd: KETTLE.cwd });
    expect(picked[0]!.runId).toBeNull();
    expect(picked[0]!.prompt).toBe("");
    // The same chat again opens the same task.
    fireEvent.click(document.querySelectorAll<HTMLElement>("[data-pick-up-continue]")[0]!);
    expect(picked[1]!.id).toBe(picked[0]!.id);
  });

  it("is hidden when this computer has no chats", async () => {
    render(
      <BridgeProvider adapter={createDemoAdapter({ latencyMs: 1, noChats: true })} core={await testCore()}>
        <PickUpWhereYouLeftOff onContinue={() => {}} />
      </BridgeProvider>,
    );
    await new Promise((r) => setTimeout(r, 50));
    expect(q("[data-pick-up]")).toBeNull();
  });
});

describe("the new setup's Ready screen", () => {
  it("offers the recent chats under the command, and Continue in Cua hands the task on", async () => {
    const picked: AskTask[] = [];
    const view = readyView({ os: "macos", agent: "claude-code", computers: ["this"] });
    render(
      <BridgeProvider adapter={createDemoAdapter({ latencyMs: 1 })} core={await testCore()}>
        <ReadyScreen view={view} agent="claude-code" place="this Mac" onDone={() => {}} onContinueChat={(t) => picked.push(t)} />
      </BridgeProvider>,
    );
    await waitFor(() => expect(document.querySelectorAll("[data-new-run=ready] [data-pick-up-row]").length).toBe(3));
    fireEvent.click(document.querySelectorAll<HTMLElement>("[data-pick-up-continue]")[0]!);
    expect(picked[0]!.imported).toMatchObject({ sessionId: KETTLE.id });
  });
});

describe.skipIf(!wasmBuilt)("a continued chat in Home", () => {
  it("shows what was said, runs nothing, and resumes the agent's own session on send", { timeout: 30000 }, async () => {
    const task = addImportedTask(KETTLE);
    const calls: { op: string; args: Record<string, unknown> }[] = [];
    await mountApp(`/home/${task.id}`, {
      failsCreates: false,
      listsGhost: false,
      demo: { onboarded: true },
      intercept: (op, args) => {
        calls.push({ op, args: args as Record<string, unknown> });
        return op === "agents.start" ? { run_id: "run-continued", agent: "claude-code" } : undefined;
      },
      answer: (op, out) => (op === "host.status" ? { ...(out as HostStatus), ...SHARED } : out),
    });
    await waitFor(() => expect(q("[data-imported-chat]")).not.toBeNull());
    // The earlier messages are read from the agent's own store when the task opens (the timeline is
    // virtualized, so jsdom draws none of its rows; the resume context below carries them).
    await waitFor(() => expect(calls.find((c) => c.op === "agents.threadHistory")?.args).toEqual({ agent: "claude-code", id: KETTLE.id }));
    expect(q("[data-earlier-heading]")?.textContent).toContain("Earlier on this Mac");
    expect(q("[data-continue-on]")?.textContent).toContain("This Mac");
    expect(q("[data-imported-note]")?.textContent).toContain("Nothing runs until you send");
    expect(calls.some((c) => c.op === "agents.start")).toBe(false);
    // Nothing goes to a Space thread: the chat stays private.
    const reply = q("[data-imported-reply]") as HTMLTextAreaElement;
    fireEvent.change(reply, { target: { value: "Pin it to UTC" } });
    fireEvent.keyDown(reply, { key: "Enter" });
    await waitFor(() => expect(calls.some((c) => c.op === "agents.start")).toBe(true));
    const start = calls.find((c) => c.op === "agents.start")!.args;
    expect(start).toMatchObject({ spaceId: "relay:this-mac-1", agent: "claude-code", prompt: "Pin it to UTC", cwd: KETTLE.cwd, resumeSession: KETTLE.id });
    expect(String(start.resumeContext)).toContain("Agent: It depends on the machine's time zone");
    expect(calls.some((c) => c.op === "thread.post")).toBe(false);
    await waitFor(() => expect(askTask(task.id)?.runId).toBeTruthy());
  });
});
