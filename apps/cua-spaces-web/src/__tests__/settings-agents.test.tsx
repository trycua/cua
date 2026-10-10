// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, AI agents, as the SwiftUI app's: each coding agent on this
// machine with what cua set up for it and Configure or Remove, "Configure
// all detected agents" in the header (the core's `agents` section over the
// host's `agents.setup`); the cua-driver card follows in its own group.

import { cleanup, fireEvent, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const section = () => document.querySelector<HTMLElement>('[data-settings-section="agents"]');
const row = (agent: string) => section()?.querySelector<HTMLElement>(`[data-setting-row="agent:${agent}"]`) ?? null;

describe.skipIf(!wasmBuilt)("Settings, AI agents", () => {
  it("lists each detected agent with its status, configures one, removes it, and configures all", { timeout: 20_000 }, async () => {
    await mountApp("/settings", { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(row("hermes")).not.toBeNull(), { timeout: 8000 });
    const s = section()!;
    expect(within(s).getByText("AI agents")).toBeTruthy();
    expect(within(row("claude-code")!).getByText("Claude Code")).toBeTruthy();
    expect(within(row("hermes")!).getByText("Not set up")).toBeTruthy();
    // Not installed: no button, greyed.
    expect(row("gemini-cli")!.getAttribute("data-enabled")).toBe("false");

    fireEvent.click(within(row("hermes")!).getByRole("button", { name: "Configure" }));
    await waitFor(() => expect(within(row("hermes")!).getByRole("button", { name: "Remove" })).toBeTruthy());
    expect(within(row("hermes")!).getByText("Skills and MCP server set up")).toBeTruthy();
    fireEvent.click(within(row("hermes")!).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(within(row("hermes")!).getByRole("button", { name: "Configure" })).toBeTruthy());

    fireEvent.click(within(s).getByRole("button", { name: "Configure all detected agents" }));
    await waitFor(() => {
      for (const a of ["hermes", "openclaw"]) expect(within(row(a)!).getByRole("button", { name: "Remove" })).toBeTruthy();
    });
  });
});
