// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The ask-first Space is prepared from launch: the home is made while the person is still on the
// first screens, a message sent while it is being made is kept and goes once when the agent is ready
// (also after the app was quit), and onboarding ends in that Space with no "Try it" screen.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { setFlagOverride } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetConnectState } from "@/hooks/use-connectors";
import { resetAskTasks } from "@/lib/ask-first";
import { getDriver } from "@/lib/prepare/store";
import { resetPrepare } from "@/lib/prepare/store";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
  resetConnectState();
  resetPrepare();
  setFlagOverride("spaces_ask_first_home", true);
  setFlagOverride("spaces_new_onboarding", true);
});
afterEach(() => {
  cleanup();
  resetPrepare();
  vi.unstubAllEnvs();
});

const q = (s: string) => document.querySelector<HTMLElement>(s);
type Call = { op: string; args: Record<string, unknown> };

async function compose(text: string) {
  const box = await waitFor(() => {
    const el = q("[data-composer]") as HTMLTextAreaElement | null;
    expect(el?.disabled).toBe(false);
    return el!;
  });
  fireEvent.change(box, { target: { value: text, selectionStart: text.length } });
  await act(async () => {
    fireEvent.keyDown(box, { key: "Enter" });
    await new Promise((r) => setTimeout(r, 20));
  });
}

describe.skipIf(!wasmBuilt)("preparing the ask-first Space from launch", () => {
  it("makes the home at launch, adds the agent and says ready", { timeout: 40000 }, async () => {
    const calls: Call[] = [];
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo: { onboarded: true }, intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined) });
    await waitFor(() => expect(getDriver()?.view()?.phase).toBe("ready"), { timeout: 20000 });
    const create = calls.find((c) => c.op === "spaces.create");
    expect(create?.args.config).toMatchObject({ name: "home-claude-code" });
    expect(calls.filter((c) => c.op === "spaces.create")).toHaveLength(1);
    expect(calls.find((c) => c.op === "thread.agentAdd")?.args).toMatchObject({ harness: "claude-code" });
    // Nothing was asked of the person.
    expect(calls.some((c) => c.op === "thread.post")).toBe(false);
  });

  it("shows the live preparing state on the home's page, keeps a message typed meanwhile and sends it once", { timeout: 60000 }, async () => {
    const calls: Call[] = [];
    let release!: () => void;
    const hold = new Promise<void>((r) => (release = r));
    const { router } = await mountApp("/spaces", { failsCreates: false, listsGhost: false, hold, demo: { onboarded: true }, intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined) });
    await waitFor(() => expect(getDriver()?.state?.pendingId).toBeTruthy());
    const pending = getDriver()!.state!.pendingId!;
    await act(async () => {
      await router.navigate({ to: "/spaces/$spaceId", params: { spaceId: pending } });
    });
    const panel = await waitFor(() => {
      const el = q("[data-prepare]");
      expect(el).not.toBeNull();
      return el!;
    });
    expect(panel.dataset.prepare).toBe("preparing");
    expect(panel.textContent).toContain("Linux runtime");
    expect(panel.textContent).toContain("Your message sends as soon as your agent is ready");
    expect(panel.textContent).not.toContain("·");
    // The box works at once: the message is kept, shown as waiting, and nothing is posted.
    await compose("fix the failing tests");
    await waitFor(() => expect(getDriver()?.state?.messages.map((m) => m.text)).toEqual(["fix the failing tests"]));
    expect(calls.some((c) => c.op === "thread.post")).toBe(false);
    await waitFor(() => expect(document.body.textContent).toContain("Sends when your agent is ready"));
    release();
    await waitFor(() => expect(getDriver()?.view()?.phase).toBe("ready"), { timeout: 30000 });
    await waitFor(() => expect(calls.filter((c) => c.op === "thread.post")).toHaveLength(1), { timeout: 15000 });
    expect(String(calls.find((c) => c.op === "thread.post")?.args.text)).toContain("fix the failing tests");
    await waitFor(() => expect(getDriver()?.state?.messages[0]?.state).toBe("delivered"));
    // Later ticks do not send it again.
    await act(async () => {
      await new Promise((r) => setTimeout(r, 1500));
    });
    expect(calls.filter((c) => c.op === "thread.post")).toHaveLength(1);
  });

  it("a message kept when the app was quit is sent once after it opens again", { timeout: 60000 }, async () => {
    const calls: Call[] = [];
    // Run 1: the create never ends; a message is typed; the page goes away (the record is in storage).
    const hold = new Promise<void>(() => {});
    const { router } = await mountApp("/spaces", { failsCreates: false, listsGhost: false, hold, demo: { onboarded: true }, intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined) });
    await waitFor(() => expect(getDriver()?.state?.pendingId).toBeTruthy());
    const pending = getDriver()!.state!.pendingId!;
    await act(async () => {
      await router.navigate({ to: "/spaces/$spaceId", params: { spaceId: pending } });
    });
    await compose("keep me");
    await waitFor(() => expect(JSON.parse(localStorage.getItem("cua.prepare.v1") ?? "{}").messages?.length).toBe(1));
    cleanup();
    resetPrepare();
    expect(calls.some((c) => c.op === "thread.post")).toBe(false);
    // Run 2: a new page load with the same storage; the create runs again and finishes.
    calls.length = 0;
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo: { onboarded: true }, intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined) });
    await waitFor(() => expect(getDriver()?.view()?.phase).toBe("ready"), { timeout: 30000 });
    await waitFor(() => expect(calls.filter((c) => c.op === "thread.post")).toHaveLength(1), { timeout: 15000 });
    expect(String(calls.find((c) => c.op === "thread.post")?.args.text)).toContain("keep me");
    expect(calls.filter((c) => c.op === "spaces.create")).toHaveLength(1);
  });

  it("does not start without the new setup, or without the new Home", { timeout: 30000 }, async () => {
    setFlagOverride("spaces_new_onboarding", false);
    const calls: Call[] = [];
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo: { onboarded: true }, intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined) });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 800));
    });
    expect(calls.some((c) => c.op === "spaces.create")).toBe(false);
    expect(getDriver()?.state?.wanted ?? false).toBe(false);
    cleanup();
    resetPrepare();
    setFlagOverride("spaces_ask_first_home", false);
    setFlagOverride("spaces_new_onboarding", true);
    await mountApp("/spaces", { failsCreates: false, listsGhost: false, demo: { onboarded: true }, intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined) });
    await act(async () => {
      await new Promise((r) => setTimeout(r, 800));
    });
    expect(calls.some((c) => c.op === "spaces.create")).toBe(false);
  });
});
