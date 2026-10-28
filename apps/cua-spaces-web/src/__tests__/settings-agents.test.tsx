// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, Connectors: each coding app on this computer with one
// switch (`agents.setup`, `agents.connect` and `agents.disconnect`),
// "Set up new apps automatically", and the sync that sets up a newly
// installed app with a notice and Undo.

import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { BridgeProvider } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { noCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { ConnectAgentDialog } from "@/components/agents/connect-dialog";
import { RunAgentDialog } from "@/components/agents/run-agent-dialog";
import { connectedAppsState, resetConnectedAppsState, setConnectedAppsState } from "@/hooks/use-connected-apps";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(() => {
  cleanup();
  localStorage.clear();
  resetConnectedAppsState();
});
stubBrowserApis();

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
      // The file path stays out of the row; it is only in the closed Advanced section.
      expect(document.querySelector("li")?.textContent).not.toContain(".claude.json");
      expect(document.querySelector<HTMLDetailsElement>("[data-connect-advanced]")?.open).toBe(false);
      expect(document.querySelector("li")?.textContent).not.toContain("Not installed");
    } else {
      await waitFor(() => expect(document.querySelector("[data-run-agent]")?.getAttribute("data-run-agent")).toBe("ready"));
    }
    mounted.unmount();
    demo.dispose?.();
  });
});

const app = (id: string) => document.querySelector<HTMLElement>(`[data-connector-app="${id}"]`);

describe.skipIf(!wasmBuilt)("Settings, Connectors", () => {
  it("keeps Set up new apps automatically in the saved state", { timeout: 20_000 }, async () => {
    await mountApp("/settings/connectors", { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(document.querySelector("[data-connected-auto]")).not.toBeNull(), { timeout: 8000 });
    expect(connectedAppsState().auto).toBeNull();
    fireEvent.click(document.querySelector("[data-connected-auto]")!);
    await waitFor(() => expect(connectedAppsState().auto).toBe(true));
    expect(JSON.parse(localStorage.getItem("cua.connectedApps")!).auto).toBe(true);
  });
});

describe.skipIf(!wasmBuilt)("a newly installed coding app", () => {
  const seen = ["claude-code", "codex", "openclaw", "cursor", "vscode"];

  it("is set up with a notice and Undo when the person said yes", { timeout: 20_000 }, async () => {
    setConnectedAppsState({ known: seen, off: [], auto: true });
    await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(document.body.textContent).toContain("Hermes can now use Cua"), { timeout: 8000 });
    expect(document.body.textContent).toContain("Restart Hermes to use it.");
    expect(connectedAppsState().known).toContain("hermes");
    fireEvent.click(await screen.findByRole("button", { name: "Undo" }));
    await waitFor(() => expect(connectedAppsState().off).toContain("hermes"));
  });

  it("is left alone without a yes, and when it was turned off", { timeout: 20_000 }, async () => {
    for (const state of [{ known: seen, off: [], auto: null }, { known: seen, off: ["hermes"], auto: true }]) {
      cleanup();
      setConnectedAppsState(state);
      const { router } = await mountApp("/settings/connectors", { failsCreates: false, listsGhost: false });
      await waitFor(() => expect(app("hermes")).not.toBeNull(), { timeout: 8000 });
      await router.load();
      expect(app("hermes")!.querySelector("[data-connector-connect]")).not.toBeNull();
      expect(document.body.textContent).not.toContain("can now use Cua");
    }
  });
});
