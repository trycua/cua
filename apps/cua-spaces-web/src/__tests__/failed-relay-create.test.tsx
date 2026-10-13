// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A macOS create on another of your machines (gamma-4) failed
// because that Mac had no Local Network access. The grid showed a second
// "Stopped, Linux" tile beside the failed one (the machine's own record of
// the half-created Space), and the toast blamed "This Mac".

import { act, cleanup, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { createFailedText } from "@/bridge";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { GHOST_ID, LOCAL_NETWORK, NAME, mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

describe.skipIf(!wasmBuilt)("a create on another of your machines that fails (real routes, wasm core)", () => {
  it("is one tile beside the machine's own record of it, and names that machine", async () => {
    let release!: () => void;
    const { hooks } = await mountApp("/spaces", { failsCreates: true, listsGhost: true, hold: new Promise<void>((r) => (release = r)) });
    await waitFor(() => expect(hooks.spaces.data?.length).toBeGreaterThan(3));
    await waitFor(() => expect(hooks.machines.data?.some((m) => m.id === "mac-mini")).toBe(true));

    // While it runs, the machine already lists its half-created Space.
    let failure: unknown;
    let created!: Promise<unknown>;
    act(() => {
      created = hooks.spaces.createSpace({ image: "ghcr.io/trycua/macos:26", os: "macos", on: "host:mac-mini", name: NAME });
    });
    const named = () => hooks.spaces.data!.filter((s) => s.name === NAME);
    await act(() => hooks.spaces.refresh());
    expect(hooks.spaces.data!.some((s) => s.id === GHOST_ID)).toBe(false);
    expect(named().map((s) => [s.id, s.status])).toEqual([[expect.stringMatching(/^pending:/), "provisioning"]]);

    // It fails: still one Space for it, the failed create's row, not also a
    // "Stopped" Linux Space on that machine.
    release();
    await act(() => created.catch((e: unknown) => (failure = e)));
    await act(() => hooks.spaces.refresh());
    const rows = named();
    expect(rows.map((s) => s.id)).toEqual([expect.stringMatching(/^pending:/)]);
    const failed = rows[0]!;
    expect(failed.progress?.error).toMatch(/^Mac mini can't reach its new VM because Cua doesn't have Local Network access/);
    expect(failed.hostName).toBe("Mac mini");
    await waitFor(() => expect(document.querySelector(`[data-space-id="${failed.id}"]`)).not.toBeNull());
    expect(document.querySelector(`[data-space-id="${GHOST_ID}"]`)).toBeNull();

    // The toast after the failure is about that machine, not this Mac.
    expect(createFailedText(hooks.core, failure)).toMatch(/^Could not create the Space: Mac mini can't reach its new VM/);
    // Without where the create ran, the words are about this Mac, as before.
    expect(createFailedText(hooks.core, new Error(LOCAL_NETWORK))).toMatch(/^Could not create the Space: This Mac can't reach/);

    // Removed from the list, the machine's own record is what is left of it.
    await act(() => hooks.spaces.dismissCreate(failed.id));
    expect(hooks.spaces.data!.filter((s) => s.name === NAME).map((s) => s.id)).toEqual([GHOST_ID]);
  });
});
