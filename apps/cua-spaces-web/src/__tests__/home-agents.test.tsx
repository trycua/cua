// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Home and the Agents page draw ONE list (`agentList.view`): a job whose
// Space is still being made and an agent that isn't connected show on both,
// with the same words and the same Connect; a row opens its Space's thread.
// Home's order: the composer, Your agents, Your Spaces.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { setFlagOverride } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { resetWakeNotes } from "@/components/space-thread/summon";
import { resetConnectedAppsState } from "@/hooks/use-connected-apps";
import { resetConnectState } from "@/hooks/use-connectors";
import { addAskTask, resetAskTasks } from "@/lib/ask-first";
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
      expect(el).toBeTruthy();
      return el!;
    },
    { timeout },
  );

async function start(job: string) {
  fireEvent.change(await screen.findByLabelText("What do you want to do?"), { target: { value: job } });
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Start" }));
  });
}

describe.skipIf(!wasmBuilt)("Your agents and the Agents page: one list", () => {
  it("a creating run and a not-connected run show on both, the same way", async () => {
    let release!: () => void;
    const hold = new Promise<void>((r) => (release = r));
    const { router } = await mountApp("/home", { failsCreates: false, listsGhost: false, hold });
    await start("fix the build");
    // While its Space is being made: on the Agents page, waiting for its Space…
    await act(async () => void router.navigate({ to: "/agents" }));
    const onAgents = await found("[data-chat-run][data-chat-tone='creating']");
    expect(onAgents.textContent).toContain("fix the build");
    expect(onAgents.querySelector("[data-chat-line]")?.textContent).toMatch(/^Waiting for .+ to be ready$/);
    const total = qa("[data-agent-list] [data-chat]").length;
    // …and Home counts the same list.
    await act(async () => void router.navigate({ to: "/home" }));
    expect((await found("[data-home-see-all='/agents']")).textContent?.trim()).toBe(`See all ${total}`);
    await act(async () => void router.navigate({ to: "/agents" }));
    release();

    // Ready, and Claude Code isn't connected: both say so, with Connect.
    const nc = await found("[data-chat-run][data-chat-tone='not-connected']", 15000);
    expect(nc.querySelector("[data-chat-line]")?.textContent).toBe("Claude Code isn't connected yet");
    expect(nc.querySelector("[data-chat-action='connect']")).not.toBeNull();
    await act(async () => void router.navigate({ to: "/home" }));
    const home = await found("[data-home-agent][data-chat-tone='not-connected']");
    expect(home.querySelector("[data-home-agent-line]")?.textContent).toBe("Claude Code isn't connected yet");
    expect(home.querySelector("[data-home-agent-action='connect']")?.textContent).toBe("Connect Claude Code");

    // A row opens its Space's thread.
    await act(async () => {
      fireEvent.click(home);
    });
    await waitFor(() => expect(router.state.location.pathname).toMatch(/^\/spaces\//));
    await found("[data-call-layout] [data-wake-note=claude]", 15000);
  });

  it("Home: the composer, then Your agents, then Your Spaces; See all links; Spaces say running", async () => {
    await mountApp("/home", { failsCreates: false, listsGhost: false });
    await start("tidy the repo");
    // Back on Home.
    fireEvent.click(await found("a[href='/home']"));
    await found("[data-home-agents]");
    await found("[data-home-spaces]");
    const order = qa("[data-ask-first-home] section, [data-ask-first-home] [aria-label='What do you want to do?']").map((el) => el.getAttribute("data-home-agents") !== null ? "agents" : el.getAttribute("data-home-spaces") !== null ? "spaces" : "composer");
    expect(order.indexOf("composer")).toBeLessThan(order.indexOf("agents"));
    expect(order.indexOf("agents")).toBeLessThan(order.indexOf("spaces"));
    expect(q("[data-home-see-all='/agents']")?.textContent?.trim()).toMatch(/^See all \d+$/);
    expect(q("[data-home-spaces] h2")?.textContent).toMatch(/running/);
    expect(q("[data-home-spaces] h2")?.textContent).not.toMatch(/\bon\b/);
    // The Spaces page's tiles.
    expect(qa("[data-home-spaces] [data-space-id]").length).toBeGreaterThan(0);
    // The composer: the agent as the thread's mention, where as a Space.
    expect(q("[data-ask-mention]")?.textContent).toBe("@claude");
    expect(q("[data-ask-where]")?.textContent).toBe("New SpaceLinux on This Mac");
  });

  it("a task whose Space is gone leaves nothing to list: Home is the welcome, not a bare box", async () => {
    addAskTask({ prompt: "an old job", agent: "claude-code", os: "linux", spaceId: "gone-space" });
    await mountApp("/home", { failsCreates: false, listsGhost: false, answer: (op, out) => (op === "spaces.list" ? [] : out) });
    expect(await screen.findByRole("heading", { name: "What do you want to do?" })).toBeTruthy();
    expect(q("[data-home-agents]")).toBeNull();
    expect(q("[data-home-spaces]")).toBeNull();
  });
});
