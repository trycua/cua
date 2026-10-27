// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Ask first with the redesign: Start goes straight to the Space being made,
// and the job is its thread's first message, @mentioning the agent. When the
// agent can't be woken (Claude Code with no Anthropic sign-in) the message
// is still posted, with "Claude Code isn't connected yet" and Connect under
// it; once connected, the message goes again. In any Space, @ offers the
// agents that can start there and sending adds one. With only one of the two
// flags, Ask first keeps its own task page (ask-first-start-failed.test.tsx).

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { setFlagOverride } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { DEMO_THREAD_SPACE } from "@/bridge/adapters/demo/thread";
import { resetWakeNotes } from "@/components/space-thread/summon";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetConnectState } from "@/hooks/use-connectors";
import { askTasks, resetAskTasks } from "@/lib/ask-first";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
  resetWakeNotes();
  resetConnectState();
  resetConnectedAppsState();
  setFlagOverride("spaces_ask_first_home", true);
});
afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
});

const q = (s: string) => document.querySelector<HTMLElement>(s);
const qa = (s: string) => [...document.querySelectorAll<HTMLElement>(s)];
const found = (s: string, timeout = 8000) =>
  waitFor(
    () => {
      const el = q(s);
      expect(el).not.toBeNull();
      return el!;
    },
    { timeout },
  );

describe.skipIf(!wasmBuilt)("Ask first in the Space (both flags)", () => {
  it("lands on the Space being made, posts @claude <job>, and offers Connect, then sends again", { timeout: 40000 }, async () => {
    const calls: { op: string; args: Record<string, unknown> }[] = [];
    const { router } = await mountApp("/home", {
      failsCreates: false,
      listsGhost: false,
      intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined),
    });
    fireEvent.change(await screen.findByLabelText("What do you want to do?"), { target: { value: "fix the build" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Start" }));
    });
    // Straight to the Space (not the task page), and its first message once made.
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/spaces\//));
    expect(router.state.location.pathname).not.toMatch(/^\/home/);
    await waitFor(() => expect(calls.some((c) => c.op === "thread.post" && c.args.text === "@claude fix the build")).toBe(true), { timeout: 15000 });
    // No Anthropic sign-in: Claude Code isn't added, and isn't started as a run either.
    expect(calls.some((c) => c.op === "thread.agentAdd")).toBe(false);
    expect(calls.some((c) => c.op === "agents.start")).toBe(false);
    expect(askTasks()[0]!.inSpace).toBe(true);
    const note = await found("[data-wake-note=claude]", 15000);
    expect(note.textContent).toContain("Claude Code isn't connected yet");
    expect(q("[data-wake-connect]")!.textContent).toContain("Connect Claude Code");
    // The message draws its mention as a chip.
    expect(qa("[data-call-layout] .text-brand-strong").some((el) => el.textContent === "@claude")).toBe(true);

    await act(async () => {
      fireEvent.click(q("[data-wake-connect]")!);
    });
    await waitFor(() => expect(q("[data-wake-note=claude]")!.dataset.state).toBe("retried"), { timeout: 15000 });
    expect(calls.filter((c) => c.op === "agents.connect").map((c) => c.args)).toEqual([{ agents: ["claude-code"], signIn: true }]);
    await waitFor(() => expect(calls.some((c) => c.op === "thread.agentAdd" && c.args.harness === "claude-code")).toBe(true));
    await waitFor(() => expect(calls.filter((c) => c.op === "thread.post" && c.args.text === "@claude fix the build")).toHaveLength(2));
  });
});

describe.skipIf(!wasmBuilt)("Send again when the agent can't join", () => {
  it("says why and posts nothing more (a message no agent reads would look sent)", { timeout: 40000 }, async () => {
    const posts: string[] = [];
    await mountApp("/home", {
      failsCreates: false,
      listsGhost: false,
      intercept: (op, args) => {
        if (op === "thread.agentAdd") throw new Error("this Space is full: remove an agent first");
        if (op === "thread.post") posts.push((args as { text: string }).text);
        return undefined;
      },
    });
    fireEvent.change(await screen.findByLabelText("What do you want to do?"), { target: { value: "fix the build" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Start" }));
    });
    await found("[data-wake-connect]", 15000);
    await act(async () => {
      fireEvent.click(q("[data-wake-connect]")!);
    });
    await waitFor(() => expect(q("[data-wake-note=claude]")!.textContent).toContain("this Space is full"), { timeout: 15000 });
    expect(q("[data-wake-note=claude]")!.dataset.state).not.toBe("retried");
    expect(posts).toEqual(["@claude fix the build"]);
  });
});

describe.skipIf(!wasmBuilt)("notes only you see", () => {
  it("say so, can be dismissed, and never reach the thread or any other member", async () => {
    const calls: { op: string; args: unknown }[] = [];
    // The create waits until the queued line has been looked at.
    let release!: () => void;
    const hold = new Promise<void>((r) => (release = r));
    await mountApp("/home", { failsCreates: false, listsGhost: false, hold, intercept: (op, args) => (calls.push({ op, args }), undefined) });
    fireEvent.change(await screen.findByLabelText("What do you want to do?"), { target: { value: "fix the build" } });
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Start" }));
    });
    // While the Space is made: the queued line says only you see it.
    const queued = await found("[data-queued-note]");
    expect(queued.hasAttribute("data-only-you")).toBe(true);
    expect(queued.textContent).toContain("Only visible to you");
    expect(queued.textContent).toContain("Sends when the Space is ready");
    release();
    // Once ready: the can't-wake note, marked the same, with Dismiss.
    const note = await found("[data-wake-note=claude]", 15000);
    expect(note.hasAttribute("data-only-you")).toBe(true);
    expect(note.querySelector("[data-only-you-label]")?.textContent).toContain("Only visible to you");
    // Nothing a note says was ever sent anywhere: not posted, not in any call.
    const sent = JSON.stringify(calls.map((c) => c.args));
    for (const words of ["Only visible to you", "isn't connected yet", "Sends when the Space is ready", "Sent again"]) expect(sent).not.toContain(words);
    expect(calls.filter((c) => c.op === "thread.post").map((c) => (c.args as { text: string }).text)).toEqual(["@claude fix the build"]);
    await act(async () => {
      fireEvent.click(note.querySelector("[data-wake-dismiss]")!);
    });
    expect(q("[data-wake-note=claude]")).toBeNull();
  });
});

describe.skipIf(!wasmBuilt)("@ any agent in a Space (redesign)", () => {
  it("offers the agents that can start here and adds one on send", async () => {
    setFlagOverride("spaces_ask_first_home", null);
    const calls: { op: string; args: Record<string, unknown> }[] = [];
    await mountApp(`/spaces/${encodeURIComponent(DEMO_THREAD_SPACE)}`, {
      failsCreates: false,
      listsGhost: false,
      intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined),
    });
    const box = await waitFor(() => {
      const el = q("[data-composer]") as HTMLTextAreaElement | null;
      expect(el?.disabled).toBe(false);
      return el!;
    });
    fireEvent.change(box, { target: { value: "@", selectionStart: 1 } });
    await found("[data-mention-picker]");
    // Codex (OpenAI is connected in the demo) starts here; its row says so.
    expect(q("[data-mention-option=codex]")!.textContent).toContain("Starts in this Space");
    fireEvent.change(box, { target: { value: "@codex bump the timeout", selectionStart: 23 } });
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter" });
      await new Promise((r) => setTimeout(r, 20));
    });
    await waitFor(() => expect(calls.some((c) => c.op === "thread.agentAdd" && c.args.name === "codex" && c.args.harness === "openai-codex")).toBe(true));
    await waitFor(() => expect(calls.some((c) => c.op === "thread.post" && c.args.text === "@codex bump the timeout")).toBe(true));
    // Added before the message, so the message wakes it.
    const add = calls.findIndex((c) => c.op === "thread.agentAdd");
    const post = calls.findIndex((c) => c.op === "thread.post" && c.args.text === "@codex bump the timeout");
    expect(add).toBeLessThan(post);
    expect(q("[data-wake-note]")).toBeNull();
  });
});
