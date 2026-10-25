// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, AI agents, as the SwiftUI app's: each coding agent on this
// machine with what cua set up for it and Configure or Remove, "Configure
// all detected agents" in the header (the core's `agents` section over the
// host's `agents.setup`); the cua-driver card follows in its own group.

import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { cleanup, fireEvent, render, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { BridgeProvider } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { noCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { ConnectAgentDialog } from "@/components/agents/connect-dialog";
import { RunAgentDialog } from "@/components/agents/run-agent-dialog";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const section = () => document.querySelector<HTMLElement>('[data-settings-section="agents"]');
const row = (agent: string) => section()?.querySelector<HTMLElement>(`[data-setting-row="agent:${agent}"]`) ?? null;

describe("configured agents without binary evidence", () => {
  it.each(["connect", "run"])("shows a configured agent as ready in the %s dialog", async (dialog) => {
    const demo = createDemoAdapter({ latencyMs: 0 });
    const adapter: typeof demo = {
      ...demo,
      call: (async (op, args) => {
        if (op === "agents.setup") return [{ agent: "claude-code", name: "Claude Code", installed: false, configured: true, detail: "skills and MCP configured", skillsInstalled: 1, skillsTotal: 1, mcpConfig: "C:/Users/test/.claude.json", skillsDir: "C:/Users/test/.claude/skills" }];
        return demo.call(op, args);
      }) as typeof demo.call,
    };
    const routeTree = createRootRoute({ component: () => dialog === "connect"
      ? <ConnectAgentDialog open onOpenChange={() => {}} />
      : <RunAgentDialog open onOpenChange={() => {}} space={{ id: "local:test", name: "Test Space", os: "windows", status: "running", detail: "", lastUsedAt: 0, scene: "blank" }} /> });
    const router = createRouter({ routeTree, history: createMemoryHistory({ initialEntries: ["/"] }) });
    const mounted = render(<BridgeProvider adapter={adapter} core={noCore}><RouterProvider router={router} /></BridgeProvider>);
    if (dialog === "connect") {
      await waitFor(() => expect(document.querySelector("li")?.textContent).toContain("Set up"));
      expect(document.querySelector("li")?.textContent).toContain("C:/Users/test/.claude.json");
      expect(document.querySelector("li")?.textContent).not.toContain("Not installed");
    } else {
      await waitFor(() => expect(document.querySelector("[data-run-agent]")?.getAttribute("data-run-agent")).toBe("ready"));
    }
    mounted.unmount();
    demo.dispose?.();
  });
});

describe.skipIf(!wasmBuilt)("Settings, AI agents", () => {
  it("lists each detected agent with its status, configures one, removes it, and configures all", { timeout: 20_000 }, async () => {
    await mountApp("/settings/connected-apps", { failsCreates: false, listsGhost: false });
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
