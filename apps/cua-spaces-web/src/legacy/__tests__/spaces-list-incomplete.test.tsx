// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.
// Before the redesign: public's test for this page, run with `spaces_redesign` off
// (the drift guard that keeps the default UI working).

// The Spaces page while the list is not complete: right after an app update
// restarts the daemon, before the first read, or while the account's other
// computers are still answering. It shows what is known and one quiet line,
// never "Create your first Space"; that only comes for a complete list with
// no Space, in the grid's place under the warnings and the filters.

import { vi as __vi } from "vitest";
__vi.stubEnv("VITE_CUA_FLAG_SPACES_REDESIGN", "0");
import { act, cleanup, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { listView } from "@/bridge";
import type { ListStatus } from "@/bridge/contracts/spaces";
import { noCore, testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "@/__tests__/app-harness";

stubBrowserApis();
afterEach(cleanup);

/** The whole app mounts and settles first (`useQuiet`). */
const WAIT = { timeout: 8000 };

/** A host whose list status the test moves, with no Space or the demo's. */
async function mount(status: { current: ListStatus }, noSpaces: boolean) {
  return mountApp("/spaces", {
    failsCreates: false,
    listsGhost: false,
    demo: { noSpaces },
    listStatus: () => status.current,
  });
}

/** The empty state: the first-run home (with the core) or its plain stand-in. */
const emptyHome = () => document.querySelector("[data-empty-home]") ?? screen.queryByText("No Spaces yet")?.closest("div") ?? null;
const reaching = () => document.querySelector("[data-spaces-reaching]")?.textContent ?? null;
const spacesCount = () => document.querySelector('a[href="/spaces"]')?.textContent ?? "";

describe("the Spaces page while the list is incomplete", { timeout: 30_000 }, () => {
  it("never shows the empty home before the first read or while computers answer", async () => {
    const status = { current: { loaded: false, incomplete: false, reaching: 0, stale: false } };
    const { emit } = await mount(status, true);
    await waitFor(() => expect(reaching()).toBe("Loading Spaces…"), WAIT);
    expect(emptyHome()).toBeNull();
    expect(screen.getByText("Loading Spaces…", { selector: "p:not([data-spaces-reaching])" })).toBeTruthy();
    // No count in the sidebar before the list was read (not "0").
    expect(spacesCount()).not.toMatch(/0/);

    // This machine answered, 6 computers have not yet.
    status.current = { loaded: true, incomplete: true, reaching: 6, stale: false };
    act(() => emit({ type: "spaces.changed" }));
    await waitFor(() => expect(reaching()).toBe("Reaching 6 computers…"), WAIT);
    expect(emptyHome()).toBeNull();

    // Every computer answered and there is no Space: now it is empty.
    status.current = { loaded: true, incomplete: false, reaching: 0, stale: false };
    act(() => emit({ type: "spaces.changed" }));
    await waitFor(() => expect(emptyHome()).not.toBeNull(), WAIT);
    expect(reaching()).toBeNull();
  });

  it("shows the Spaces known so far with the line under them", async () => {
    const status = { current: { loaded: true, incomplete: true, reaching: 1, stale: false } };
    await mount(status, false);
    await waitFor(() => expect(reaching()).toBe("Reaching 1 computer…"), WAIT);
    const grid = document.querySelector("ul.grid");
    expect(grid?.children.length).toBeGreaterThan(0);
    // The line follows the tiles, so they do not move when it goes.
    expect(grid!.compareDocumentPosition(document.querySelector("[data-spaces-reaching]")!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(emptyHome()).toBeNull();
  });

  it("puts the empty home in the grid's place, under the filters", async () => {
    await mount({ current: { loaded: true, incomplete: false, reaching: 0, stale: false } }, true);
    await waitFor(() => expect(emptyHome()).not.toBeNull(), WAIT);
    const filters = document.querySelector('[aria-label="Filter by state"]')!;
    expect(filters.compareDocumentPosition(emptyHome()!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  });
});

describe("listView", () => {
  // The core's decision, and the TypeScript stand-in without it, agree.
  const cases: [ListStatus, number, { empty: boolean; reachingLine: string | null }][] = [
    [{ loaded: false, incomplete: false, reaching: 0, stale: false }, 0, { empty: false, reachingLine: "Loading Spaces…" }],
    [{ loaded: true, incomplete: true, reaching: 6, stale: false }, 0, { empty: false, reachingLine: "Reaching 6 computers…" }],
    [{ loaded: true, incomplete: true, reaching: 1, stale: false }, 2, { empty: false, reachingLine: "Reaching 1 computer…" }],
    [{ loaded: true, incomplete: true, reaching: 0, stale: false }, 0, { empty: false, reachingLine: "Reaching your computers…" }],
    [{ loaded: true, incomplete: false, reaching: 0, stale: false }, 0, { empty: true, reachingLine: null }],
    [{ loaded: true, incomplete: false, reaching: 0, stale: false }, 3, { empty: false, reachingLine: null }],
  ];
  it.each(cases)("%o with %i Spaces", async (status, count, want) => {
    expect(listView(noCore, status, count)).toEqual(want);
    if (wasmBuilt) expect(listView(await testCore(), status, count)).toEqual(want);
  });
});
