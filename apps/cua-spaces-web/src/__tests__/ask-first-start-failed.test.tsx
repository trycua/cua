// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The "Ask first" Home sent a task to "This computer" and agents.start got
// the pseudo-id `this-mac`: the core answered with its own jargon
// ("use its id, such as local:<name> ... add_space") and the page said
// "Stopped" over a screen that said "Finished". A start that fails now says
// "Didn't start" everywhere, in plain words, with Change computer and Try
// again in the card and the core's words under Details.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { setFlagOverride } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { addAskTask, askTask, resetAskTasks } from "@/lib/ask-first";

import { mountApp, stubBrowserApis } from "./app-harness";

const CORE_NOT_FOUND = 'Space "this-mac" (use its id, such as local:<name> or cloud:<name>, or add it with add_space) not found';

afterEach(cleanup);
stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetAskTasks();
  setFlagOverride("spaces_ask_first_home", true);
});

describe.skipIf(!wasmBuilt)("a task that couldn't start (real routes, wasm core)", () => {
  it("says Didn't start in plain words, and Change computer starts it on a new Linux computer", async () => {
    const task = addAskTask({ prompt: "Hey!", agent: "openai-codex", os: "macos", spaceId: "this-mac", error: CORE_NOT_FOUND });
    const { router } = await mountApp(`/home/${task.id}`, { failsCreates: false, listsGhost: false });
    await screen.findByText("Agents can't work on this Mac itself");
    expect(router.state.location.pathname).toBe(`/home/${task.id}`);
    // The header and the screen agree.
    expect(screen.getAllByText("Didn't start").length).toBeGreaterThanOrEqual(2);
    expect(screen.queryByText("Finished")).toBeNull();
    expect(screen.queryByText("Stopped")).toBeNull();
    // Jargon only under Details.
    const details = screen.getByText("Details").closest("details")!;
    expect(details.textContent).toContain("add_space");
    expect(document.querySelector("[data-ask-start-failed]")!.textContent!.replace(details.textContent!, "")).not.toMatch(/add_space|local:</);
    // Try again can't help a target that can't work: only Change computer.
    expect(screen.queryByRole("button", { name: /Try again/ })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /Change computer/ }));
    const linux = await screen.findByRole("menuitem", { name: /A new Linux computer on this Mac/ });
    expect(screen.getByText(/Agents don't work on this Mac itself/)).toBeTruthy();
    expect(screen.queryByRole("menuitem", { name: /This computer/ })).toBeNull();
    await act(async () => {
      fireEvent.click(linux);
    });
    await waitFor(() => expect(askTask(task.id)?.runId).toBeTruthy(), { timeout: 5000 });
    expect(askTask(task.id)).toMatchObject({ error: null, os: "linux" });
    expect(askTask(task.id)?.spaceId).not.toBe("this-mac");
  });

  it("offers Try again for an error that may pass", async () => {
    const task = addAskTask({ prompt: "Run the tests", agent: "claude-code", os: "linux", spaceId: "local:agent-sandbox", error: "agent-sandbox is not running" });
    await mountApp(`/home/${task.id}`, { failsCreates: false, listsGhost: false });
    await screen.findByText("Couldn't reach that computer");
    expect(screen.getByRole("button", { name: /Try again/ })).toBeTruthy();
    expect(screen.getByRole("button", { name: /Change computer/ })).toBeTruthy();
  });
});
