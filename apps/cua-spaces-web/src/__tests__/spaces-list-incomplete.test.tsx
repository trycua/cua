// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Your computers while the list is not complete: right after an app update
// restarts the daemon, before the first read, or while the account's other
// computers are still answering. The page shows what is known and one quiet
// line, never the empty list; that only comes for a complete list.

import { act, cleanup, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { listView } from "@/bridge";
import type { ListStatus } from "@/bridge/contracts/spaces";
import { noCore, testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "./app-harness";

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

const empty = () => document.querySelector("[data-tree-empty]");
const reaching = () => document.querySelector("[data-spaces-reaching]")?.textContent ?? null;
const spacesCount = () => document.querySelector('a[href="/spaces"]')?.textContent ?? "";

describe("your computers while the list is incomplete", { timeout: 30_000 }, () => {
  it("never shows the empty list before the first read or while computers answer", async () => {
    const status = { current: { loaded: false, incomplete: false, reaching: 0, stale: false } };
    const { emit } = await mount(status, true);
    await waitFor(() => expect(reaching()).toBe("Loading Spaces…"), WAIT);
    expect(empty()).toBeNull();
    // No count in the sidebar before the list was read (not "0").
    expect(spacesCount()).not.toMatch(/0/);

    // This computer answered, 6 computers have not yet.
    status.current = { loaded: true, incomplete: true, reaching: 6, stale: false };
    act(() => emit({ type: "spaces.changed" }));
    await waitFor(() => expect(reaching()).toBe("Reaching 6 computers…"), WAIT);
    expect(empty()).toBeNull();

    // Every computer answered: the line goes.
    status.current = { loaded: true, incomplete: false, reaching: 0, stale: false };
    act(() => emit({ type: "spaces.changed" }));
    await waitFor(() => expect(reaching()).toBeNull(), WAIT);
  });

  it("shows the computers and Spaces known so far with the line under them", async () => {
    const status = { current: { loaded: true, incomplete: true, reaching: 1, stale: false } };
    await mount(status, false);
    await waitFor(() => expect(reaching()).toBe("Reaching 1 computer…"), WAIT);
    const list = document.querySelector("[data-spaces-tree]");
    expect(list?.querySelectorAll("[data-tree-row]").length).toBeGreaterThan(0);
    // The line follows the list, so the rows do not move when it goes.
    expect(list!.compareDocumentPosition(document.querySelector("[data-spaces-reaching]")!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(empty()).toBeNull();
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
