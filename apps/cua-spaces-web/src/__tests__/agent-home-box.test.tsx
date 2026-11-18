// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// With the Ask first and Child Spaces experiments on, the box has no where row: the job goes to the agent's Home
// Space (the slim Linux image), which makes the Spaces it needs. A place is named with @.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetConnectState } from "@/hooks/use-connectors";
import { AGENT_HOME_IMAGE } from "@/lib/agent-home/container";
import { resetGrants } from "@/lib/agent-home/grants";
import { resetHomes } from "@/lib/agent-home/home-space";
import { askTasks, resetAskTasks } from "@/lib/ask-first";
import { resetPickedAgent } from "@/lib/ask-first-model";
import { resetRepos } from "@/lib/ask-first-repo";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
  resetPickedAgent();
  resetRepos();
  resetHomes();
  resetGrants();
  resetConnectState();
  resetConnectedAppsState();
});
afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
});

const q = (s: string) => document.querySelector<HTMLElement>(s);
const HOME = { failsCreates: false, listsGhost: false, demo: { agentHome: true, accounts: true } };

async function useClaude() {
  await act(async () => {
    fireEvent.click(await waitFor(() => q('[data-picker="agent"]')!));
  });
  await act(async () => {
    fireEvent.click(await waitFor(() => q('[data-agent-group="claude-code"] [data-agent-account="kwork"]')!));
  });
}

describe.skipIf(!wasmBuilt)("the box with an agent home", () => {
  it("shows no where row until a place is named with @", async () => {
    await mountApp("/home/ask", HOME);
    const box = (await screen.findByLabelText("What should it do?")) as HTMLTextAreaElement;
    await waitFor(() => expect(q("[data-ask-agent]")!.textContent).toContain("GPT-5.5"));
    expect(q('[data-picker="where"]')).toBeNull();
    expect(q("[data-ask-where]")).toBeNull();
    fireEvent.change(box, { target: { value: "run the tests on @" } });
    await waitFor(() => expect(q('[data-picker="where"]')).not.toBeNull());
  });

  it("sends the job to the agent's Home Space on the slim image, with the spawn capability and a briefing", async () => {
    const calls: { op: string; args: Record<string, unknown> }[] = [];
    const { router } = await mountApp("/home/ask", {
      ...HOME,
      intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined),
    });
    const box = (await screen.findByLabelText("What should it do?")) as HTMLTextAreaElement;
    await waitFor(() => expect(q("[data-ask-agent]")!.textContent).toContain("GPT-5.5"));
    fireEvent.change(box, { target: { value: "test my project on a Mac and a Windows computer" } });
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter" });
    });
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/spaces\//));
    const create = await waitFor(() => {
      const c = calls.find((x) => x.op === "spaces.create");
      expect(c).toBeTruthy();
      return c!;
    });
    const config = (create.args as { config: Record<string, unknown> }).config;
    expect(config).toMatchObject({ image: AGENT_HOME_IMAGE, children: true, name: expect.stringMatching(/^[a-z]+-[a-z]+$/) });
    expect(config.labels).toMatchObject({ "cua.home": "1", "cua.home.agent": "openai-codex" });
    expect(AGENT_HOME_IMAGE).toBe("ghcr.io/trycua/linux:24.04-slim");
    expect(config).not.toHaveProperty("headless");
    // The agent is a member kept on standby with the model and the briefing; the job is the thread's first message.
    await waitFor(
      () =>
        expect(
          calls.some(
            (c) =>
              c.op === "thread.agentAdd" &&
              c.args.standby === true &&
              c.args.harness === "openai-codex" &&
              typeof c.args.model === "string" &&
              String(c.args.brief).includes("your own small Linux Space"),
          ),
        ).toBe(true),
      { timeout: 15000 },
    );
    await waitFor(
      () => expect(calls.some((c) => c.op === "thread.post" && c.args.text === "@codex test my project on a Mac and a Windows computer")).toBe(true),
      { timeout: 15000 },
    );
    await waitFor(() => expect(askTasks()[0]).toMatchObject({ home: true, inSpace: true }));
  });

  it("starts a new Home for a Home message, and continues the existing one only when it is picked", async () => {
    const calls: { op: string; args: Record<string, unknown> }[] = [];
    const { router } = await mountApp("/home/ask", {
      ...HOME,
      intercept: (op, args) => (calls.push({ op, args: args as Record<string, unknown> }), undefined),
    });
    const box = (await screen.findByLabelText("What should it do?")) as HTMLTextAreaElement;
    await waitFor(() => expect(q("[data-ask-agent]")!.textContent).toContain("GPT-5.5"));
    // The demo's Home was made for Claude Code (its label says so): the next request goes there.
    await useClaude();
    await waitFor(() => expect(q("[data-ask-agent]")!.textContent).toBe("Sonnet 5.5High"));
    // The tree is read from the host and the Home is found by its label.
    await new Promise((r) => setTimeout(r, 300));
    // A plain Home message makes a new Home, even though the agent already has one.
    fireEvent.change(box, { target: { value: "run the tests" } });
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter" });
    });
    await waitFor(() => expect(calls.some((c) => c.op === "spaces.create")).toBe(true));
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/spaces\/(?!local%3Aagent-home-demo)/));
    // Picking "in your existing Home" continues it: no second create, the job goes to the existing Home.
    await act(async () => {
      await router.navigate({ to: "/home/ask" });
    });
    calls.length = 0;
    const box2 = (await screen.findByLabelText("What should it do?")) as HTMLTextAreaElement;
    fireEvent.change(box2, { target: { value: "run the tests on @" } });
    await act(async () => {
      fireEvent.click(await waitFor(() => q('[data-picker="where"]')!));
    });
    await act(async () => {
      fireEvent.click(await screen.findByText("Agent decides, same Home"));
    });
    fireEvent.change(box2, { target: { value: "run the tests again" } });
    await act(async () => {
      fireEvent.keyDown(box2, { key: "Enter" });
    });
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/spaces\/local%3A([a-z]+-[a-z]+|agent-home-demo)/));
    expect(calls.some((c) => c.op === "spaces.create")).toBe(false);
    await waitFor(() => expect(calls.some((c) => c.op === "thread.post" && c.args.text === "@claude run the tests again")).toBe(true), { timeout: 15000 });
  });

  it("names the Space's handle for the Home's agent, never the host's unique name for it", async () => {
    // `cua agent ls` says claude-8 (the host made it unique: other Spaces have a claude); the Space calls it @claude.
    const calls: { op: string; args: Record<string, unknown> }[] = [];
    const member = { name: "claude-8", handle: "claude", harness: "claude-code", status: "idle", activity: null, sessions: 1 };
    const { router } = await mountApp("/home/ask", {
      ...HOME,
      intercept: (op, args) => {
        calls.push({ op, args: args as Record<string, unknown> });
        return op === "thread.members" ? { members: [member] } : undefined;
      },
    });
    const box = (await screen.findByLabelText("What should it do?")) as HTMLTextAreaElement;
    await waitFor(() => expect(q("[data-ask-agent]")!.textContent).toContain("GPT-5.5"));
    await useClaude();
    await waitFor(() => expect(q("[data-ask-agent]")!.textContent).toBe("Sonnet 5.5High"));
    await new Promise((r) => setTimeout(r, 300));
    fireEvent.change(box, { target: { value: "hi" } });
    await act(async () => {
      fireEvent.keyDown(box, { key: "Enter" });
    });
    // A Home message makes a new Home; the handle is still the Space's own.
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/spaces\//));
    await waitFor(() => expect(calls.some((c) => c.op === "thread.post" && c.args.text === "@claude hi")).toBe(true), { timeout: 15000 });
    // Adding the standby agent names the handle too (it is the same member), and nothing is posted to @claude-8.
    expect(calls.filter((c) => c.op === "thread.agentAdd").every((c) => c.args.name === "claude")).toBe(true);
    expect(calls.some((c) => c.op === "thread.post" && String(c.args.text).includes("claude-8"))).toBe(false);
  });
});
