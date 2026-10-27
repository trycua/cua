// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.
// Before the redesign: public's test for this page, run with `spaces_redesign` off
// (the drift guard that keeps the default UI working).

// The Agents page's persistent-agent detail, as the SwiftUI app's: the run's
// conversation, then the core's Memory, Routines and "Access to this
// computer" tabs (the access audit under Access).

import { vi as __vi } from "vitest";
__vi.stubEnv("VITE_CUA_FLAG_SPACES_REDESIGN", "0");
import { cleanup, fireEvent, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "@/__tests__/app-harness";

afterEach(cleanup);
stubBrowserApis();

const tab = (name: string) => document.querySelector<HTMLElement>(`[data-agent-tab="${name}"]`);
const panel = (name: string) => document.querySelector<HTMLElement>(`[data-agent-panel="${name}"]`);

describe.skipIf(!wasmBuilt)("Agents, a persistent agent's detail", () => {
  it("shows Memory, adds a routine and lists Access with its audit", { timeout: 20_000 }, async () => {
    await mountApp("/agents?agent=ada", { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(tab("memory")).not.toBeNull(), { timeout: 8000 });
    expect([...document.querySelectorAll("[data-agent-tab]")].map((t) => t.textContent)).toEqual(["Conversation", "Memory", "Routines", "Access to this computer"]);
    expect(tab("run")!.getAttribute("aria-selected")).toBe("true");

    fireEvent.click(tab("memory")!);
    await waitFor(() => expect(within(panel("memory")!).getByText("notes/review-checklist.md")).toBeTruthy());
    fireEvent.click(within(panel("memory")!).getByText("MEMORY.md"));
    await waitFor(() => expect(panel("memory")!.querySelector("[data-agent-file-text]")?.textContent).toContain("light and dark"));
    expect(within(panel("memory")!).getAllByRole("button", { name: "Restore" })).toHaveLength(2);
    fireEvent.click(within(panel("memory")!).getByRole("button", { name: "Close" }));

    fireEvent.click(tab("routines")!);
    const routines = await waitFor(() => panel("routines")!);
    expect(within(routines).getByText("Morning design notes")).toBeTruthy();
    expect(within(routines).getByText("Every day at 9:00 AM")).toBeTruthy();
    const add = within(routines).getByRole("button", { name: "Add routine" }) as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    fireEvent.change(routines.querySelector("[data-routine-title]")!, { target: { value: "Inbox" } });
    fireEvent.change(routines.querySelector("[data-routine-prompt]")!, { target: { value: "Sort my inbox" } });
    await waitFor(() => expect(add.disabled).toBe(false));
    fireEvent.click(add);
    await waitFor(() => expect(within(panel("routines")!).getByText("Inbox")).toBeTruthy());
    expect(within(panel("routines")!).getByText("Every day at 8:00 AM")).toBeTruthy();

    fireEvent.click(tab("access")!);
    const access = await waitFor(() => panel("access")!);
    expect(within(access).getByText("This computer")).toBeTruthy();
    expect(within(access).getByRole("button", { name: "Revoke" })).toBeTruthy();
    expect(within(access).getByText("Audit")).toBeTruthy();
    expect(within(access).getByText("Used relay:this-mac")).toBeTruthy();
  });
});
