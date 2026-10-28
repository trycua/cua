// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// CUA-2644: an agent that runs on another machine (the customer's Hermes on Linux, driving their
// MacBook). Connect says which machine it set up and which it did not touch, and has an "On another
// machine" block with the host, the exact commands, the permissions and the reload note. The first
// run offers "An agent on another machine", which opens the join sheet.

import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { BridgeProvider } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { ConnectAgentDialog } from "@/components/agents/connect-dialog";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const q = (sel: string) => document.querySelector<HTMLElement>(sel);

async function mountDialog() {
  const adapter = createDemoAdapter({ latencyMs: 0 });
  const routeTree = createRootRoute({ component: () => <ConnectAgentDialog open onOpenChange={() => {}} /> });
  const router = createRouter({ routeTree, history: createMemoryHistory({ initialEntries: ["/"] }) });
  render(
    <BridgeProvider adapter={adapter} core={await testCore()}>
      <RouterProvider router={router} />
    </BridgeProvider>,
  );
  return adapter;
}

describe.skipIf(!wasmBuilt)("an agent on another machine", () => {
  it("Connect names the machine it edits, and its On another machine block has the host, commands, permissions and reload note", async () => {
    await mountDialog();
    const here = await waitFor(() => {
      const h = q("[data-connect-here]");
      if (!h || !/^On \S/.test(h.textContent ?? "") || /this machine$/.test(h.textContent ?? "") === false) throw new Error("not named yet");
      return h;
    });
    const name = here.textContent!.replace(/^On /, "").replace(/this machine$/, "");
    const block = q("[data-connect-other-machine]")!;
    expect(screen.getByRole("heading", { name: "On another machine" })).toBeTruthy();
    expect(block.textContent).toContain(`Host to name: ${name}`);
    // The exact commands for the chosen agent: the same account, then the agent's setup.
    expect(block.textContent).toContain("cua auth login");
    expect(block.textContent).toContain("cua agents setup --agents hermes --yes");
    fireEvent.click(within(block).getByRole("button", { name: "Codex" }));
    expect(block.textContent).toContain("cua agents setup --agents codex --yes");
    expect(q("[data-connect-other-permissions]")?.textContent).toMatch(/share its desktop/);
    expect(q("[data-connect-other-reload]")?.textContent).toMatch(/Restart the agent/);
    expect(block.textContent).not.toMatch(/·|\bdevices?\b/i);
  });

  it("after Set up it says which machine it configured and which others it did not touch", async () => {
    await mountDialog();
    const setUp = await screen.findByRole("button", { name: /^Set up \d+ agent/ }, { timeout: 5000 }).catch(() => null);
    if (setUp) await act(async () => fireEvent.click(setUp));
    await waitFor(() => expect(q("[data-connect-done]")).not.toBeNull(), { timeout: 5000 });
    const done = q("[data-connect-done]")!;
    expect(done.textContent).toMatch(/^Set up on \S/);
    // The demo account has other machines: they are named, and Connect says it left them alone.
    expect(q("[data-connect-others]")?.textContent).toMatch(/Not touched: .+/);
  });

  it("Settings, Connectors names the machine it edits, what it leaves alone, and shows how to use it from another machine", async () => {
    await mountApp("/settings/connectors", { failsCreates: false, listsGhost: false });
    const footer = await waitFor(() => {
      const f = q("[data-connectors-machine]");
      if (!f) throw new Error("no footer");
      return f;
    });
    expect(footer.textContent).toMatch(/^On \S+/);
    // The demo account has other machines: named, and left alone.
    expect(footer.textContent).toMatch(/Not touched: .+/);
    expect(q("[data-connect-other-machine]")).toBeNull();
    fireEvent.click(q("[data-connectors-other-toggle]")!);
    const block = await waitFor(() => {
      const b = q("[data-connect-other-machine]");
      if (!b) throw new Error("not open");
      return b;
    });
    expect(block.textContent).toContain("cua auth login");
    expect(block.textContent).toContain("cua agents setup --agents hermes --yes");
    expect(q("[data-connect-other-permissions]")?.textContent).toMatch(/share its desktop/);
    expect(q("[data-connect-other-reload]")?.textContent).toMatch(/Restart the agent/);
    expect(block.textContent).not.toMatch(/·|\bdevices?\b/i);
    // An app's own page says which machine it is on.
    cleanup();
    await mountApp("/settings/connectors/claude-code", { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(q("[data-connector-on]")?.textContent).toMatch(/^On \S+/));
  });

  it("the first run offers An agent on another machine, which opens the join sheet", async () => {
    await mountApp("/onboarding?demo=fresh", { failsCreates: false, listsGhost: false });
    const skip = await screen.findByRole("button", { name: "Get started" });
    fireEvent.click(skip);
    const skipSignIn = await screen.findByRole("button", { name: "Skip", exact: true } as never).catch(() => null);
    if (skipSignIn) fireEvent.click(skipSignIn);
    const row = await waitFor(() => {
      const r = q("[data-first-run-other-machine]");
      if (!r) throw new Error("not on the connect step yet");
      return r;
    }, { timeout: 8000 });
    expect(row.textContent).toContain("An agent on another machine");
    fireEvent.click(q("[data-first-run-other-machine-open]")!);
    await waitFor(() => expect(q('[data-add-agent="connect"]')).not.toBeNull());
  });
});
