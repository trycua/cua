// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Computers tab has exactly one "Add a computer" in every state: the
// header's button when there are computers (only This Mac, or several), the
// empty state's own "Add another computer" when there are none. A card that
// offers a second button for the same thing fails here.

import { cleanup, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import type { MachineRow } from "@/bridge/contracts/host";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

stubBrowserApis();
afterEach(cleanup);

const WAIT = { timeout: 8000 };
const ADD = /^add (a|another) computer$/i;

/** Every control on the tab that opens the add-a-computer sheet. */
function addEntries(): string[] {
  const byLabel = [...document.querySelectorAll<HTMLElement>("button, a, [role=button]")].filter((e) => ADD.test((e.textContent ?? "").trim()));
  const marked = [...document.querySelectorAll<HTMLElement>("[data-tree-add-computer]")];
  return [...new Set([...byLabel, ...marked])].map((e) => (e.textContent ?? "").trim());
}

const host = (over: Partial<TestHost>): TestHost => ({ failsCreates: false, listsGhost: false, ...over });
const here = (rows: MachineRow[]) => rows.filter((m) => m.current || m.via === "local");

describe("the Computers tab has one Add a computer in every state", { timeout: 30_000 }, () => {
  it("only This Mac: the header button, and the card is a hint with no button", async () => {
    await mountApp("/spaces", host({ demo: { noSpaces: true }, answer: (op, out) => (op === "machines.list" ? here(out as MachineRow[]) : out) }));
    await waitFor(() => expect(document.querySelector("[data-tree-single]")).not.toBeNull(), WAIT);
    expect(document.querySelector("[data-tree-single] button, [data-tree-single] a")).toBeNull();
    expect(addEntries()).toEqual(["Add a computer"]);
  });

  it("several computers: the header button only", async () => {
    await mountApp("/spaces", host({}));
    await waitFor(() => expect(document.querySelectorAll("[data-tree-kind=computer]").length).toBeGreaterThan(1), WAIT);
    expect(document.querySelector("[data-tree-single]")).toBeNull();
    expect(addEntries()).toEqual(["Add a computer"]);
  });

  it("no computers yet: one Add another computer beside Add this Mac", async () => {
    await mountApp("/spaces", host({ demo: { noSpaces: true }, answer: (op, out) => (op === "machines.list" ? [] : out) }));
    await waitFor(() => expect(document.querySelector("[data-tree-empty]")).not.toBeNull(), WAIT);
    expect(addEntries()).toEqual(["Add another computer"]);
  });
});
