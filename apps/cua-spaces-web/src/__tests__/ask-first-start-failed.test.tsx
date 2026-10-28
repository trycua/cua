// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The "Ask first" Home sent a task to "This computer" and agents.start got
// the pseudo-id `this-mac`: the core answered with its own jargon and the
// page said "Stopped" over a screen that said "Finished". This Mac is now a
// real target, asked about once: it starts on the Space id its host
// service shares it as. A start that fails says "Didn't start" in plain
// words, with Try again and Change computer in the card.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { setFlagOverride } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { addAskTask, askTask, askTasks, resetAskTasks, setThisMacConfirmed, thisMacConfirmed } from "@/lib/ask-first";

import { mountApp, stubBrowserApis } from "./app-harness";

const CORE_NOT_FOUND = 'Space "this-mac" (use its id, such as local:<name> or cloud:<name>, or add it with add_space) not found';
/** The demo host shares this Mac on the relay as machine `this-mac`. */
const HOST_SPACE = "relay:this-mac";
const HOST = { failsCreates: false, listsGhost: false };

afterEach(() => {
  cleanup();
  vi.unstubAllEnvs();
});
stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
  setFlagOverride("spaces_ask_first_home", true);
  // Ask first on its own: its task page (with the redesign too, see ask-first-in-space.test.tsx).
  vi.stubEnv("VITE_CUA_FLAG_SPACES_REDESIGN", "0");
});

async function pickThisMac() {
  fireEvent.click(document.querySelector('[data-picker="where"]')!);
  const item = await screen.findByRole("menuitem", { name: /^This Mac/ });
  await act(async () => {
    fireEvent.click(item);
  });
}

async function startJob(text: string) {
  fireEvent.change(screen.getByLabelText("What do you want to do?"), { target: { value: text } });
  await act(async () => {
    fireEvent.click(screen.getByRole("button", { name: "Start" }));
  });
}

describe.skipIf(!wasmBuilt)("This Mac as a task's computer (real routes, wasm core)", () => {
  it("defaults to a new Linux computer, and This Mac asks once, then starts on this Mac's Space id", async () => {
    await mountApp("/home", HOST);
    await screen.findByText("What do you want to do?");
    expect(document.querySelector('[data-picker="where"]')!.textContent).toContain("A new Linux computer");
    await pickThisMac();
    await screen.findByText("Work on this Mac?");
    expect(screen.getByText(/Claude Code will work directly on this Mac, with your files and apps\. It can't be undone by throwing the computer away\./)).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Use this Mac" }));
    });
    expect(thisMacConfirmed()).toBe(true);
    await waitFor(() => expect(document.querySelector('[data-picker="where"]')!.textContent).toContain("This Mac"));
    await startJob("Tidy my Downloads folder");
    const id = askTasks()[0]!.id;
    await waitFor(() => expect(askTask(id)?.runId).toBeTruthy(), { timeout: 5000 });
    expect(askTask(id)).toMatchObject({ spaceId: HOST_SPACE, onHost: true, error: null });
  });

  it("Pick another keeps the default and reopens the picker", async () => {
    await mountApp("/home", HOST);
    await screen.findByText("What do you want to do?");
    await pickThisMac();
    await act(async () => {
      fireEvent.click(await screen.findByRole("button", { name: "Pick another" }));
    });
    await screen.findByRole("menuitem", { name: /A new Linux computer on this Mac/ });
    expect(thisMacConfirmed()).toBe(false);
    expect(document.querySelector('[data-picker="where"]')!.textContent).toContain("A new Linux computer");
  });

  it("doesn't ask again once answered on this device", async () => {
    setThisMacConfirmed(true);
    await mountApp("/home", HOST);
    await screen.findByText("What do you want to do?");
    await pickThisMac();
    await waitFor(() => expect(document.querySelector('[data-picker="where"]')!.textContent).toContain("This Mac"));
    expect(screen.queryByText("Work on this Mac?")).toBeNull();
  });
});

describe.skipIf(!wasmBuilt)("a task that couldn't start (real routes, wasm core)", () => {
  it("an old this-mac task says Didn't start in plain words, and Try again starts it on this Mac", async () => {
    setThisMacConfirmed(true);
    const task = addAskTask({ prompt: "Hey!", agent: "openai-codex", os: "macos", spaceId: "this-mac", error: CORE_NOT_FOUND });
    await mountApp(`/home/${task.id}`, HOST);
    await screen.findByText("Couldn't start on this Mac");
    // The header and the screen agree.
    expect(screen.getAllByText("Didn't start").length).toBeGreaterThanOrEqual(2);
    expect(screen.queryByText("Finished")).toBeNull();
    expect(screen.queryByText("Stopped")).toBeNull();
    // Jargon only under Details.
    const details = screen.getByText("Details").closest("details")!;
    expect(details.textContent).toContain("add_space");
    expect(document.querySelector("[data-ask-start-failed]")!.textContent!.replace(details.textContent!, "")).not.toMatch(/add_space|local:</);
    expect(screen.getByRole("button", { name: /Change computer/ })).toBeTruthy();
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /Try again/ }));
    });
    await waitFor(() => expect(askTask(task.id)?.runId).toBeTruthy(), { timeout: 5000 });
    expect(askTask(task.id)).toMatchObject({ spaceId: HOST_SPACE, onHost: true, error: null });
  });

  it("Change computer moves a failed task to a new Linux computer", async () => {
    const task = addAskTask({ prompt: "Run the tests", agent: "claude-code", os: "linux", spaceId: "local:gone", error: 'Space "local:gone" (use its id, such as local:<name> or cloud:<name>, or add it with add_space) not found' });
    await mountApp(`/home/${task.id}`, HOST);
    await screen.findByText("Couldn't find that computer");
    expect(screen.queryByRole("button", { name: /Try again/ })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /Change computer/ }));
    const linux = await screen.findByRole("menuitem", { name: /A new Linux computer on this Mac/ });
    await act(async () => {
      fireEvent.click(linux);
    });
    await waitFor(() => expect(askTask(task.id)?.runId).toBeTruthy(), { timeout: 5000 });
    expect(askTask(task.id)).toMatchObject({ error: null, os: "linux", onHost: false });
  });

  it("offers Try again for an error that may pass", async () => {
    const task = addAskTask({ prompt: "Run the tests", agent: "claude-code", os: "linux", spaceId: "local:agent-sandbox", error: "agent-sandbox is not running" });
    await mountApp(`/home/${task.id}`, HOST);
    await screen.findByText("Couldn't reach that computer");
    expect(screen.getByRole("button", { name: /Try again/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Change computer/ })).toBeTruthy();
  });
});
