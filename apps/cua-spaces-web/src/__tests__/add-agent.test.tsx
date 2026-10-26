// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// "Add an agent" (CUA-2264): "+" in In this Space opens the sheet. Start an
// agent here adds one inside the Space; Connect your own agent shows one
// line to copy, waits for the agent and closes when it joins, offers a new
// code when it expires, cancels a code nobody used, and turns into a join
// request in the thread when "Ask me before it joins" is on.

import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { BridgeProvider } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { DEMO_THREAD_SPACE } from "@/bridge/adapters/demo/thread";
import { testCore } from "@/bridge/__tests__/testCore";
import { threadOpAvailable } from "@/bridge/ops/thread";
import { AddAgentSheet, freeHandle } from "@/components/add-agent/add-agent-sheet";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const q = (s: string) => document.querySelector<HTMLElement>(s);
const qa = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
const path = `/spaces/${encodeURIComponent(DEMO_THREAD_SPACE)}`;
const LINE = 'Join my Cua Space "ubuntu-build": https://cua.ai/join/7KQ2-9FXA';

let clipboard: string[] = [];
beforeEach(() => {
  clipboard = [];
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText: vi.fn(async (t: string) => void clipboard.push(t)) } });
});

const app = (demo: Record<string, unknown> = {}) => mountApp(path, { failsCreates: false, listsGhost: false, demo });
const click = async (el: HTMLElement) => {
  await act(async () => {
    fireEvent.click(el);
    await new Promise((r) => setTimeout(r, 20));
  });
};
const found = (s: string, timeout = 4000) =>
  waitFor(
    () => {
      const el = q(s);
      expect(el).not.toBeNull();
      return el!;
    },
    { timeout },
  );

/** Opens Add an agent and goes to one of its rows. */
async function openSheet(row?: "start" | "connect") {
  await click(await found("[data-add-agent-open]"));
  await found("[data-add-agent='pick']");
  if (row) await click(q(`[data-add-row='${row}']`)!);
}
const line = () => q("[data-join-line]") as HTMLTextAreaElement | null;

describe("Add an agent", () => {
  it("+ opens two rows, Share with a person, and the marks of the agents that work", async () => {
    await app({ joinRedeemMs: null });
    await openSheet();
    expect(q("[data-add-agent='pick']")?.textContent).toContain("Add an agent to ubuntu-build");
    // Share with a person leads back to Share, where the page can share.
    expect(qa("[data-add-row]").map((r) => r.dataset.addRow)).toEqual(["start", "connect", "share"]);
    expect(q("[data-add-row='share']")?.textContent).toContain("Share with a person");
    expect(qa("[data-add-agent-marks] [data-harness-mark]").map((m) => m.dataset.harnessMark)).toEqual(["hermes", "claude-code", "codex", "cursor", "openclaw", "t3code"]);
  });

  it("Start an agent here: Hermes, Claude Code, Codex, Cursor, then OpenClaw; a free name; it joins asleep", async () => {
    await app({ joinRedeemMs: null });
    await openSheet("start");
    expect(qa("[data-start-harness]").map((h) => h.dataset.startHarness)).toEqual(["hermes", "claude-code", "codex", "cursor", "openclaw"]);
    // Hermes is taken: the name moves on.
    expect((q("[data-start-name]") as HTMLInputElement).value).toBe("hermes-2");
    await click(q("[data-start-harness='openclaw']")!);
    expect((q("[data-start-name]") as HTMLInputElement).value).toBe("openclaw");
    expect(q("[data-start-agent]")?.textContent).toBe("Start OpenClaw");
    await click(q("[data-start-agent]")!);
    await waitFor(() => expect(q("[data-add-agent]")).toBeNull());
    const row = await found("[data-member='agent:openclaw']");
    expect(row.dataset.memberStatus).toBe("asleep");
    expect(row.querySelector("[data-owner-badge]")?.getAttribute("data-owner-badge")).toBe("acct_ada");
    await found("[data-member-note='joined'][data-member-handle='openclaw']");
  });

  it("Start refuses a name that is taken, in one line", async () => {
    await app({ joinRedeemMs: null });
    await openSheet("start");
    fireEvent.change(q("[data-start-name]")!, { target: { value: "claude" } });
    await click(q("[data-start-agent]")!);
    expect((await found("[data-start-error]")).textContent).toBe("@claude is taken here. Pick another name.");
    expect(q("[data-add-agent='start']")).not.toBeNull();
  });

  it("Connect: one read-only line with Copy; copied, it waits, and closes when the agent joins", async () => {
    await app({ joinRedeemMs: 600 });
    await openSheet("connect");
    await waitFor(() => expect(line()?.value).toBe(LINE));
    expect(line()?.readOnly).toBe(true);
    expect(q("[data-join-doc]")?.dataset.joinDoc).toBe("https://cua.ai/join/7KQ2-9FXA");
    expect(q("[data-join-waiting]")).toBeNull();
    await click(q("[data-prompt-copy]")!);
    expect(clipboard).toEqual([LINE]);
    expect(q("[data-prompt-copy]")?.textContent).toBe("Copied");
    expect(q("[data-join-waiting]")?.textContent).toBe("Waiting for your agent to run the code…");
    expect(q("[data-join-done]")).not.toBeNull();
    // Ada's Codex runs it: the sheet closes by itself, the join note names her computer.
    await waitFor(() => expect(q("[data-add-agent]")).toBeNull(), { timeout: 5000 });
    const note = await found("[data-member-note='joined'][data-member-handle='codex']");
    expect(note.textContent).toContain("Codex joined. Ada connected it from Ada's MacBook Pro.");
    expect((await found("[data-member='agent:codex']")).textContent).toContain("Ada's MacBook Pro");
  });

  it("an expired code offers a new one", async () => {
    await app({ joinRedeemMs: null, joinExpireMs: 300 });
    await openSheet("connect");
    await waitFor(() => expect(line()?.value).toBe(LINE));
    await click(q("[data-prompt-copy]")!);
    await found("[data-join-expired]", 5000);
    expect(q("[data-prompt-copy]")?.hasAttribute("disabled")).toBe(true);
    await click(q("[data-join-renew]")!);
    await waitFor(() => expect(line()?.value).toMatch(/^Join my Cua Space "ubuntu-build": https:\/\/cua\.ai\/join\/[2-9A-Z]{4}-[2-9A-Z]{4}$/));
    expect(line()?.value).not.toBe(LINE);
    expect(q("[data-join-expired]")).toBeNull();
  });

  it("Cancel cancels the unused code; a changed setting makes a new one", async () => {
    const { demo } = await app({ joinRedeemMs: null });
    await openSheet("connect");
    await waitFor(() => expect(line()?.value).toBe(LINE));
    // "Ask me before it joins": the old code is cancelled, a new one made.
    await click(q("[data-join-ask]")!);
    await waitFor(() => expect(line()?.value).not.toBe(LINE));
    await click(q("[data-join-cancel]")!);
    await waitFor(() => expect(q("[data-add-agent]")).toBeNull());
    const { codes } = await demo.call("thread.joinCodeList", { spaceId: DEMO_THREAD_SPACE });
    expect(codes.map((c) => [c.state, c.askBeforeJoin])).toEqual([
      ["cancelled", true],
      ["cancelled", false],
    ]);
  });

  it("Ask me before it joins: the agent asks in the thread, and Let it join brings it in", async () => {
    await app({ joinRedeemMs: 300 });
    await openSheet("connect");
    await waitFor(() => expect(line()?.value).toBe(LINE));
    await click(q("[data-join-ask]")!);
    await waitFor(() => expect(line()?.value).not.toBe(LINE));
    await click(q("[data-prompt-copy]")!);
    await waitFor(() => expect(q("[data-add-agent]")).toBeNull(), { timeout: 5000 });
    const req = await found("[data-join-request-state='open']");
    expect(req.textContent).toContain("Ada connected Codex from Ada's MacBook Pro. It will read this thread and take tasks from members.");
    expect(req.textContent).toContain("Wants to join");
    expect(q("[data-member='agent:codex']")).toBeNull();
    await click(req.querySelector<HTMLElement>("[data-join-request-allow]")!);
    await found("[data-join-request-state='approved']");
    expect(q("[data-join-request-answer]")?.textContent).toBe("Ada let it join");
    await found("[data-member='agent:codex']");
  });

  it("someone else's agent asking to join: the owner lets it in or denies it", async () => {
    await app({ joinRedeemMs: null, threadRequest: true });
    const req = await found("[data-join-request-state='open']");
    expect(req.textContent).toContain("Linus connected OpenClaw from linus-thinkpad.");
    expect(req.querySelector("[data-owner-badge]")?.getAttribute("data-owner-badge")).toBe("acct_linus");
    await click(req.querySelector<HTMLElement>("[data-join-request-deny]")!);
    await found("[data-join-request-state='denied']");
    expect(q("[data-member='agent:openclaw']")).toBeNull();
  });

  it("a guest has no +; a member can add", async () => {
    await app({ threadRole: "guest" });
    await found("[data-members]");
    await waitFor(() => expect(q("[data-composer-off]")).not.toBeNull());
    expect(q("[data-add-agent-open]")).toBeNull();
    cleanup();
    await app({ threadRole: "member" });
    await found("[data-add-agent-open]");
  });

  it("the SwiftUI and Electron hosts route the join ops; the Tauri shell shows the rows as coming", () => {
    expect(threadOpAvailable("demo", "thread.joinCodeCreate")).toBe(true);
    expect(threadOpAvailable("webkit", "thread.joinCodeCreate")).toBe(true);
    expect(threadOpAvailable("electron", "thread.agentAdd")).toBe(true);
    expect(threadOpAvailable("tauri", "thread.joinCodeCreate")).toBe(false);
    expect(threadOpAvailable("tauri", "thread.read")).toBe(false);
    expect(threadOpAvailable("webkit", "thread.read")).toBe(true);
    expect(freeHandle("codex", ["codex", "codex-2"])).toBe("codex-3");
  });
});

describe("the sheet, reused with a Space picker", () => {
  it("puts the Space in the title as a picker (the Agents tab's New agent)", async () => {
    const demo = createDemoAdapter({ latencyMs: 0, stepMs: 0 });
    render(
      <BridgeProvider adapter={demo} core={await testCore()}>
        <AddAgentSheet open onOpenChange={() => {}} space={{ id: "a", name: "ubuntu-build" }} spaces={[{ id: "a", name: "ubuntu-build" }, { id: "b", name: "design-review" }]} onSpaceChange={() => {}} role="owner" />
      </BridgeProvider>,
    );
    await found("[data-add-agent='pick']");
    expect(q("[data-add-agent='pick'] [aria-label='Space']")?.textContent).toContain("ubuntu-build");
  });
});
