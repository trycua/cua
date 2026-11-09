// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// CUA-3319: "Pick up where you left off". Continue in Cua opens the chat in a
// private Home task with its earlier messages; nothing runs until the person
// sends, and then the agent resumes its own session on this Mac, in the
// chat's folder.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { BridgeProvider, type DataAdapter, type HostStatus } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { ContinueOn } from "@/components/ask-first/imported-task";
import { recentThreads } from "@/components/ask-first/pick-up";
import { PickUpWhereYouLeftOff, ReadyScreen } from "@/components/first-run-new/first-run-new";
import { readyView } from "@/lib/first-run-new";
import { addImportedTask, askTask, resetAskTasks, type AskTask } from "@/lib/ask-first";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
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

/** The demo host, with `agents.recentThreads` answered by `list`. */
function listing(list: (args: unknown) => Promise<unknown>): DataAdapter {
  const base = createDemoAdapter({ latencyMs: 1 });
  return new Proxy(base, {
    get: (t, k) => (k === "call" ? (op: string, args: unknown) => (op === "agents.recentThreads" ? list(args) : (t.call as (o: string, a: unknown) => Promise<unknown>)(op, args)) : Reflect.get(t, k)),
  }) as DataAdapter;
}

describe("the new setup's Ready screen", () => {
  const rows = () => [...document.querySelectorAll<HTMLElement>("[data-new-run=ready] [data-pick-up-row]")].map((r) => r.dataset.pickUpRow);
  const ready = async (agent: string, adapter: DataAdapter = createDemoAdapter({ latencyMs: 1 }), onContinueChat: (t: AskTask) => void = () => {}) =>
    render(
      <BridgeProvider adapter={adapter} core={await testCore()}>
        <ReadyScreen view={readyView({ os: "macos", agent, computers: ["this"] })} agent={agent} place="this Mac" onDone={() => {}} onContinueChat={onContinueChat} />
      </BridgeProvider>,
    );

  it("offers the picked agent's recent chats under the command, and Continue in Cua hands the task on", async () => {
    const picked: AskTask[] = [];
    await ready("claude-code", undefined, (t) => picked.push(t));
    await waitFor(() => expect(rows()).toEqual(["claude-code"]));
    fireEvent.click(document.querySelectorAll<HTMLElement>("[data-pick-up-continue]")[0]!);
    expect(picked[0]!.imported).toMatchObject({ sessionId: KETTLE.id });
  });

  it("lists only Codex's chats when Codex was picked", async () => {
    await ready("codex");
    await waitFor(() => expect(rows()).toEqual(["openai-codex"]));
  });

  it("lists both agents' chats, newest first, when both are asked for", async () => {
    render(
      <BridgeProvider adapter={createDemoAdapter({ latencyMs: 1 })} core={await testCore()}>
        <PickUpWhereYouLeftOff agents={["claude-code", "codex"]} onContinue={() => {}} />
      </BridgeProvider>,
    );
    await waitFor(() => expect(document.querySelectorAll("[data-pick-up-row]").length).toBe(2));
    expect([...document.querySelectorAll<HTMLElement>("[data-pick-up-row]")].map((r) => r.dataset.pickUpRow)).toEqual(["claude-code", "openai-codex"]);
  });

  it("asks the host for the picked agent only, holds the place quietly while it reads, and hides when it has none", async () => {
    const asked: unknown[] = [];
    let answer: (v: unknown) => void = () => {};
    const adapter = listing((args) => (asked.push(args), new Promise((r) => (answer = r))));
    await ready("claude-code", adapter);
    await waitFor(() => expect(asked).toEqual([{ limit: 4, agents: ["claude-code"] }]));
    expect(q("[data-pick-up][data-pick-up-loading]")).toBeTruthy();
    expect(document.querySelectorAll("[data-pick-up-placeholder]").length).toBe(2);
    expect(document.querySelector("[role=progressbar], .animate-spin")).toBeNull();
    answer([]);
    await waitFor(() => expect(q("[data-pick-up]")).toBeNull());
  });

  it("shows nothing for an agent whose chats Cua doesn't read, and asks nothing", async () => {
    const asked: unknown[] = [];
    const adapter = listing((args) => (asked.push(args), Promise.resolve([])));
    await ready("gemini-cli", adapter);
    await new Promise((r) => setTimeout(r, 20));
    expect(q("[data-pick-up]")).toBeNull();
    expect(asked).toEqual([]);
  });

  it("uses the list read while the agent was being connected: no second ask, no placeholder", async () => {
    const asked: unknown[] = [];
    const adapter = listing((args) => (asked.push(args), Promise.resolve([{ agent: "claude-code", id: KETTLE.id, title: KETTLE.title, cwd: KETTLE.cwd, lastActiveMs: KETTLE.lastActiveMs, resume: [] }])));
    await recentThreads(adapter, ["claude-code"]);
    await ready("claude-code", adapter);
    expect(rows()).toEqual(["claude-code"]);
    expect(q("[data-pick-up-placeholder]")).toBeNull();
    expect(asked).toHaveLength(1);
  });
});

describe("Continue on", () => {
  it("lists only your other computers that could run the chat, each once, and fits the window", async () => {
    const demo = createDemoAdapter({ latencyMs: 1 });
    const extra = [
      { id: "browser-1", name: "Web browser", via: "relay", online: true, os: "web", current: false, device: true, limits: [] },
      { id: "browser-2", name: "Web browser", via: "relay", online: true, os: "web", current: false, device: true, limits: [] },
    ];
    const adapter: DataAdapter = {
      ...demo,
      subscribe: (l) => demo.subscribe(l),
      call: (async (op: string, args: never) => {
        const out = await demo.call(op as never, args);
        if (op !== "machines.list") return out;
        const rows = out as { id: string }[];
        // The Mac mini listed twice, plus two signed-in browsers.
        return [...rows, rows.find((r) => r.id === "mac-mini")!, ...extra];
      }) as DataAdapter["call"],
    };
    render(
      <BridgeProvider adapter={adapter} core={await testCore()}>
        <ContinueOn />
      </BridgeProvider>,
    );
    fireEvent.click(await waitFor(() => q("[data-continue-on]")!));
    await waitFor(() => expect(q("[data-continue-on-menu]")).not.toBeNull());
    await waitFor(() => expect(document.querySelectorAll("[data-continue-on-other]").length).toBeGreaterThan(0));
    const ids = [...document.querySelectorAll<HTMLElement>("[data-continue-on-other]")].map((e) => e.dataset.continueOnOther);
    // Mac mini once, the Linux box; no browsers, and Studio PC is offline.
    expect(ids).toEqual(["mac-mini", "linux-box"]);
    expect(document.body.textContent).not.toContain("Web browser");
    expect(q("[data-continue-on-offline]")?.textContent).toBe("1 more computer is offline.");
    // It scrolls inside the room the window has rather than running off it.
    const menu = q("[data-continue-on-menu]")!;
    expect(menu.className).toContain("max-h-[min(480px,var(--available-height))]");
    expect(menu.className).toContain("overflow-y-auto");
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
    await waitFor(() => expect(calls.find((c) => c.op === "agents.threadHistory")?.args).toEqual({ agent: "claude-code", id: KETTLE.id, limit: 30 }));
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

describe.skipIf(!wasmBuilt)("a long chat in Home", () => {
  it("opens on its newest messages and reads more only when asked", { timeout: 30000 }, async () => {
    const task = addImportedTask(KETTLE);
    const calls: { op: string; args: Record<string, unknown> }[] = [];
    await mountApp(`/home/${task.id}`, {
      failsCreates: false,
      listsGhost: false,
      demo: { onboarded: true },
      intercept: (op, args) => {
        calls.push({ op, args: args as Record<string, unknown> });
        return undefined;
      },
      answer: (op, out) => (op === "agents.threadHistory" ? { ...(out as object), earlierOmitted: 1 } : out),
    });
    await waitFor(() => expect(q("[data-earlier-more]")).not.toBeNull());
    expect(calls.filter((c) => c.op === "agents.threadHistory").map((c) => c.args.limit)).toEqual([30]);
    fireEvent.click(q("[data-earlier-more]")!);
    await waitFor(() => expect(calls.filter((c) => c.op === "agents.threadHistory").map((c) => c.args.limit)).toEqual([30, 130]));
  });
});
