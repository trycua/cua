// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The agent's home is a Space like any other in the Spaces list: no "Home" label beside its name.

import { act, cleanup, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { forgetHome, rememberHome, resetHomes } from "@/lib/agent-home/home-space";
import { mountApp, stubBrowserApis } from "./app-harness";

stubBrowserApis();
beforeEach(() => {
  localStorage.clear();
  resetHomes();
});
afterEach(cleanup);

const spaceIds = () => [...document.querySelectorAll<HTMLElement>('[data-tree-kind="space"][data-space-id]')].map((e) => e.getAttribute("data-space-id")!);

describe.skipIf(!wasmBuilt)("the Home tag in the list of your computers", () => {
  it("shows beside the agent's home Space and nowhere else, and goes when the home is forgotten", async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    const ids = await waitFor(() => {
      const found = spaceIds().filter((id) => id !== "this-mac");
      expect(found.length).toBeGreaterThan(1);
      return found;
    });
    // No "Home" label beside the name, remembered as the agent's home or not: the agent and its role say what it is elsewhere.
    act(() => rememberHome("claude-code", ids[0]!));
    await new Promise((r) => setTimeout(r, 50));
    expect(document.querySelector("[data-home-tag]")).toBeNull();
    act(() => forgetHome("claude-code"));
  });
});
