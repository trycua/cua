// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// "Remove from list" on a failed create on another machine
// ended on another Space's page. It leaves for the Spaces grid, in place of
// the page it was on, and nothing else decides where the person lands.

import { act, cleanup, fireEvent, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { NAME, mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const after = (ms: number) => new Promise((r) => setTimeout(r, ms));

describe.skipIf(!wasmBuilt)("Remove from list on a failed create (real routes, wasm core)", () => {
  it("returns to the Spaces grid and never opens another Space", async () => {
    const { router, hooks } = await mountApp("/spaces");
    await waitFor(() => expect(hooks.spaces.data?.length).toBeGreaterThan(3));
    await waitFor(() => expect(hooks.machines.data?.some((m) => m.id === "mac-mini")).toBe(true));
    await act(async () => {
      await hooks.spaces.createSpace({ image: "ghcr.io/trycua/macos:26", os: "macos", on: "host:mac-mini", name: NAME }).catch(() => {});
    });
    const failed = hooks.spaces.data!.find((s) => s.name === NAME && s.id.startsWith("pending:"))!;
    expect(failed.progress?.error).toBeTruthy();

    // Open the failed Space's page, as a click on its tile does.
    await act(() => router.navigate({ to: "/spaces/$spaceId", params: { spaceId: failed.id } }));
    fireEvent.click(await screen.findByRole("button", { name: "Remove from list" }));

    await waitFor(() => expect(hooks.spaces.data!.some((s) => s.id === failed.id)).toBe(false));
    // Give any late navigation (a follow, a retry) time to happen.
    await after(150);
    expect(router.state.location.pathname).toBe("/spaces");
    // The page is not left behind in the history: Back never returns to a Space that is gone.
    act(() => router.history.back());
    await after(50);
    expect(router.state.location.pathname).toBe("/spaces");
    expect(screen.queryByText("This Space isn't here")).toBeNull();
  });

  it("a page follows the Space its creating row becomes, but not from the address of a row that is gone", async () => {
    const { router, hooks } = await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(hooks.spaces.data?.length).toBeGreaterThan(3));

    // Open on the creating row: it follows the Space it becomes.
    let created!: Promise<{ id: string }>;
    act(() => {
      created = hooks.spaces.createSpace({ image: "ghcr.io/trycua/linux:24.04", os: "linux", name: "follows" });
    });
    const row = await waitFor(() => {
      const r = hooks.spaces.data!.find((s) => s.id.startsWith("pending:"));
      expect(r).toBeDefined();
      return r!;
    });
    await act(() => router.navigate({ to: "/spaces/$spaceId", params: { spaceId: row.id } }));
    await screen.findByRole("heading", { name: "follows" });
    const space = await act(() => created);
    await waitFor(() => expect(router.state.location.pathname).toBe(`/spaces/${encodeURIComponent(space.id)}`), { timeout: 3000 });
    expect(hooks.spaces.createdId(row.id)).toBe(space.id);

    // The address of that row again (Back after it was removed, an old link):
    // that page did not show it, so it says the Space isn't there and stays.
    await act(() => router.navigate({ to: "/spaces/$spaceId", params: { spaceId: row.id } }));
    await screen.findByText("This Space isn't here");
    await after(100);
    expect(router.state.location.pathname).toBe(`/spaces/${encodeURIComponent(row.id)}`);
  });
});
