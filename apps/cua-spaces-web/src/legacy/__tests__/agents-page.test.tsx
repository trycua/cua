// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.
// Before the redesign: public's test for this page, run with `spaces_redesign` off
// (the drift guard that keeps the default UI working).

// The Agents page: always-on agents under Running, recent runs with Watch or
// Replay, a run that needs the person, and the empty page by what is true
// on this computer.

import { vi as __vi } from "vitest";
__vi.stubEnv("VITE_CUA_FLAG_SPACES_REDESIGN", "0");
import { cleanup, fireEvent, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "@/__tests__/app-harness";

afterEach(cleanup);
stubBrowserApis();

const section = (id: string) => document.querySelector<HTMLElement>(`[data-agents-section="${id}"]`);
const noRuns = (op: string) => (op === "agents.list" || op === "agents.runs" ? [] : undefined);
const host = { failsCreates: false, listsGhost: false } as const;

describe.skipIf(!wasmBuilt)("Agents page", () => {
  it("shows always-on agents under Running and the runs below, without a Connect button", { timeout: 20_000 }, async () => {
    await mountApp("/agents", host);
    await waitFor(() => expect(section("running")).not.toBeNull(), { timeout: 8000 });
    expect(within(section("running")!).getByText("ada")).toBeTruthy();
    expect(document.querySelector("[data-agent-view]")).toBeNull();
    expect(section("running")!.compareDocumentPosition(section("runs")!) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(document.body.textContent).not.toContain("Connect an agent");
    expect(document.querySelector("[data-connected-apps-link]")).not.toBeNull();
    const buttons = [...section("runs")!.querySelectorAll("[data-run-open]")].map((b) => b.textContent);
    expect(buttons).toContain("Watch");
    expect(buttons).toContain("Replay");
  });

  it("offers Open Space for a run that needs you, in its own section", { timeout: 20_000 }, async () => {
    await mountApp("/agents", host);
    await waitFor(() => expect(section("waiting")).not.toBeNull(), { timeout: 8000 });
    expect(within(section("waiting")!).getByText("OpenClaw needs you to sign in to GitHub")).toBeTruthy();
    expect(within(section("waiting")!).getByRole("button", { name: "Open Space" })).toBeTruthy();
    // No reply to a waiting agent from the app yet: nothing else to press.
    expect(within(section("waiting")!).getAllByRole("button")).toHaveLength(1);
  });

  it("opens a run and goes back to the overview", { timeout: 20_000 }, async () => {
    await mountApp("/agents", host);
    await waitFor(() => expect(section("runs")).not.toBeNull(), { timeout: 8000 });
    fireEvent.click(section("runs")!.querySelector("[data-run-open]")!);
    await waitFor(() => expect(document.querySelector("[data-agent-view]")).not.toBeNull());
    fireEvent.click(document.querySelector("[data-agents-back]")!);
    await waitFor(() => expect(section("runs")).not.toBeNull());
  });

  it("Set up opens Connectors with the app in view when coding apps are found but Cua is not in them", { timeout: 20_000 }, async () => {
    const { router } = await mountApp("/agents", { ...host, intercept: noRuns, demo: { agents: "unset" } });
    await waitFor(() => expect(document.querySelector("[data-agents-empty]")).not.toBeNull(), { timeout: 8000 });
    await waitFor(() => expect(document.querySelector("[data-agents-empty]")?.getAttribute("data-agents-empty")).toBe("setup"), { timeout: 8000 });
    const setup = await waitFor(() => {
      const b = document.querySelector("[data-agents-setup]");
      expect(b).not.toBeNull();
      return b!;
    }, { timeout: 8000 });
    fireEvent.click(setup);
    // Cua connects an app only where the person says so: Connectors, not in place.
    await waitFor(() => expect(router.state.location.pathname).toBe("/settings/connectors"), { timeout: 8000 });
    expect(router.state.location.search).toHaveProperty("app");
  });

  it("teaches the first prompts when Cua is already in the apps", { timeout: 20_000 }, async () => {
    await mountApp("/agents", { ...host, intercept: noRuns });
    await waitFor(() => expect(document.querySelector('[data-agents-empty="ready"]')).not.toBeNull(), { timeout: 8000 });
    expect(document.querySelectorAll("[data-agent-prompt]")).toHaveLength(3);
  });

  it("says no coding app was found", { timeout: 20_000 }, async () => {
    await mountApp("/agents", { ...host, intercept: noRuns, demo: { agents: "none" } });
    await waitFor(() => expect(document.querySelector('[data-agents-empty="none"]')).not.toBeNull(), { timeout: 8000 });
    expect(document.querySelectorAll("[data-agent-prompt]")).toHaveLength(0);
  });
});
