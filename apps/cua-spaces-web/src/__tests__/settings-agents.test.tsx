// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, Connected apps: each coding app on this computer with one
// switch (`agents.setup`, `agents.configure` and the host's own row press),
// "Set up new apps automatically", and the sync that sets up a newly
// installed app with a notice and Undo.

import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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

const app = (id: string) => document.querySelector<HTMLElement>(`[data-connected-app="${id}"]`);
const switchOf = (id: string) => within(app(id)!).getByRole("switch");

describe.skipIf(!wasmBuilt)("Settings, Connected apps", () => {
  it("lists the coding apps on this computer with one switch each, file paths only under Advanced", { timeout: 20_000 }, async () => {
    await mountApp("/settings/connected-apps", { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(app("hermes")).not.toBeNull(), { timeout: 8000 });
    expect(within(app("claude-code")!).getByText("Claude Code")).toBeTruthy();
    expect(switchOf("claude-code").getAttribute("aria-checked")).toBe("true");
    expect(switchOf("hermes").getAttribute("aria-checked")).toBe("false");
    // No file paths, skill counts or "Not installed" rows until Show all.
    expect(document.body.textContent).not.toMatch(/\.claude\.json|Not installed|skills/);
    expect(document.querySelector('[data-connected-missing="gemini-cli"]')).toBeNull();
    fireEvent.click(document.querySelector("[data-connected-show-all]")!);
    await waitFor(() => expect(document.querySelector('[data-connected-missing="gemini-cli"]')).not.toBeNull());
    // Rows stay icon, name and switch; paths and Add it by hand wait in a closed Advanced.
    expect(app("claude-code")!.textContent).not.toContain(".claude.json");
    const advanced = document.querySelector<HTMLDetailsElement>("[data-connected-advanced]")!;
    expect(advanced.open).toBe(false);
    expect(advanced.querySelector('[data-connected-file="claude-code"]')!.textContent).toContain("~/.claude.json");
    expect(advanced.querySelector("[data-connected-by-hand]")).not.toBeNull();
  });

  it("switches one app on and off, remembers an app turned off, and offers Undo", { timeout: 20_000 }, async () => {
    await mountApp("/settings/connected-apps", { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(app("hermes")).not.toBeNull(), { timeout: 8000 });
    fireEvent.click(switchOf("hermes"));
    await waitFor(() => expect(switchOf("hermes").getAttribute("aria-checked")).toBe("true"));
    fireEvent.click(switchOf("hermes"));
    await waitFor(() => expect(switchOf("hermes").getAttribute("aria-checked")).toBe("false"));
    expect(connectedAppsState().off).toContain("hermes");
    // The notice says what happened and how to take it back.
    const undo = await screen.findByRole("button", { name: "Undo" });
    expect(document.body.textContent).toContain("Hermes turned off");
    expect(document.body.textContent).toContain("Cua took itself out of Hermes's settings");
    fireEvent.click(undo);
    await waitFor(() => expect(switchOf("hermes").getAttribute("aria-checked")).toBe("true"));
    expect(connectedAppsState().off).not.toContain("hermes");
  });

  it("says why a setup failed in plain words, and offers Try again and the file to edit by hand", { timeout: 20_000 }, async () => {
    await mountApp("/settings/connected-apps", { failsCreates: false, listsGhost: false, demo: { agentsReadOnly: ["hermes"] } });
    await waitFor(() => expect(app("hermes")).not.toBeNull(), { timeout: 8000 });
    fireEvent.click(switchOf("hermes"));
    const failure = await waitFor(() => {
      const f = document.querySelector<HTMLElement>('[data-connected-failure="hermes"]');
      expect(f).not.toBeNull();
      return f!;
    });
    expect(failure.textContent).toContain("Couldn't set up Hermes. Cua can't change this file because it is read-only or belongs to another user.");
    expect(document.body.textContent).not.toMatch(/os error/);
    expect(switchOf("hermes").getAttribute("aria-checked")).toBe("false");
    expect(within(failure).getByRole("button", { name: "Try again" })).toBeTruthy();
    fireEvent.click(within(failure).getByRole("button", { name: "Add it by hand" }));
    expect(failure.textContent).toContain("mcp_servers:");
    expect(failure.textContent).toContain("Paste this into the file, then restart Hermes.");
  });

  it("keeps Set up new apps automatically in the saved state", { timeout: 20_000 }, async () => {
    await mountApp("/settings/connected-apps", { failsCreates: false, listsGhost: false });
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
      const { router } = await mountApp("/settings/connected-apps", { failsCreates: false, listsGhost: false });
      await waitFor(() => expect(app("hermes")).not.toBeNull(), { timeout: 8000 });
      await router.load();
      expect(switchOf("hermes").getAttribute("aria-checked")).toBe("false");
      expect(document.body.textContent).not.toContain("can now use Cua");
    }
  });
});
