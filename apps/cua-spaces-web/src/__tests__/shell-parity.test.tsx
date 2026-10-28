// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The shell as the SwiftUI main window has it (real routes, the wasm core,
// the demo host): the enroll bar above every page, the power button on a
// Space's tile, and the Keyvault's Settings rows the host reports.

import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { cleanup, fireEvent, render, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { BridgeProvider } from "@/bridge";
import type { DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter, type DemoOptions } from "@/bridge/adapters/demo";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { ToastProvider } from "@/components/ui/toast";
import { TooltipProvider } from "@/components/ui/tooltip";
import { routeTree } from "../routeTree.gen";
import { stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

async function mount(path: string, options: DemoOptions = {}) {
  const demo = createDemoAdapter({ latencyMs: 1, stepMs: 4, ...options });
  const calls: [string, unknown][] = [];
  const adapter: DataAdapter = {
    mode: demo.mode,
    subscribe: (l) => demo.subscribe(l),
    call: (async (op: string, args: unknown) => {
      calls.push([op, args]);
      return (demo.call as (o: string, a: unknown) => Promise<unknown>)(op, args);
    }) as DataAdapter["call"],
  };
  const router = createRouter({ routeTree, history: createMemoryHistory({ initialEntries: [path] }) });
  render(
    <BridgeProvider adapter={adapter} core={await testCore()} storeOptions={{ tickMs: 5 }}>
      <TooltipProvider delay={500}>
        <ToastProvider>
          <RouterProvider router={router} />
        </ToastProvider>
      </TooltipProvider>
    </BridgeProvider>,
  );
  return { router, calls };
}

const found = <T extends Element>(selector: string) =>
  waitFor(() => {
    const el = document.querySelector<T>(selector);
    if (!el) throw new Error(`no ${selector}`);
    return el;
  });

describe.skipIf(!wasmBuilt)("the shell as the SwiftUI main window (real routes, wasm core)", () => {
  it("shows the trust bar above every page while this machine is not trusted, and its button opens the sheet", async () => {
    const { router } = await mount("/spaces", { unenrolled: true });
    const bar = await found<HTMLElement>("[data-shell-devices-banner]");
    expect(bar.textContent).toMatch(/Trust this machine/);

    await router.navigate({ to: "/keyvault" });
    await found("[data-shell-devices-banner]");

    fireEvent.click(await found("[data-shell-banner-enroll]"));
    await waitFor(() => expect(router.state.location.pathname).toBe("/machines"));
    await found("[data-enroll]");
    // This machine's page says it under its own name: no second bar there.
    expect(document.querySelector("[data-shell-devices-banner]")).toBeNull();

    fireEvent.click(await found('[data-enroll-option="sign-in"]'));
    await waitFor(() => expect(document.querySelector('[data-machine-trust="enrolled"]')).toBeTruthy());
    await router.navigate({ to: "/spaces" });
    await found("[data-space-id]");
    expect(document.querySelector("[data-shell-devices-banner]")).toBeNull();
  });

  it("shows no enroll bar for an enrolled device", async () => {
    await mount("/spaces");
    await found("[data-space-id]");
    await new Promise((r) => setTimeout(r, 50));
    expect(document.querySelector("[data-shell-devices-banner]")).toBeNull();
  });

  it("gives a Space's tile the core's power button, and pressing it turns the Space off", async () => {
    const { calls } = await mount("/spaces");
    const tile = (await found<HTMLElement>('[data-space-id][data-state="running"]')).parentElement!;
    const power = await waitFor(() => {
      const b = tile.querySelector<HTMLButtonElement>("[data-tile-power]");
      if (!b) throw new Error("no power button");
      return b;
    });
    expect(power.getAttribute("data-tile-power")).toBe("off");
    expect(["Suspend", "Turn off"]).toContain(power.title);
    fireEvent.click(power);
    await waitFor(() => expect(calls.some(([op, a]) => op === "spaces.setPower" && (a as { on: boolean }).on === false)).toBe(true));
  });

  it("lays out every Keyvault option the host reports, on the Keyvault page, and changes its switches", async () => {
    const { calls } = await mount("/keyvault", { macHost: true });
    fireEvent.click(await found<HTMLElement>("[data-keyvault-options]"));
    const dialog = await found<HTMLElement>("[data-keyvault-options-dialog]");
    const row = (id: string) => dialog.querySelector<HTMLElement>(`[data-setting-row="${id}"]`);
    await waitFor(() => expect(row("keyvault-unlock-prompt")).toBeTruthy());
    expect(row("keyvault-auto-wipe")).toBeTruthy();
    expect(row("keyvault-site-icons")).toBeTruthy();
    expect(row("keyvault-protection:Unlock")?.textContent).toContain("Mac login");
    // Plain words, not the core's.
    expect(within(row("keyvault-auto-wipe")!).getByText("Remove saved sign-ins from Spaces after an hour")).toBeTruthy();

    fireEvent.click(within(row("keyvault-unlock-prompt")!).getByRole("switch"));
    await waitFor(() => expect(calls).toContainEqual(["settings.choose", { row: "keyvault-unlock-prompt", option: "off" }]));
    await waitFor(() => expect(within(row("keyvault-unlock-prompt")!).getByRole("switch").getAttribute("aria-checked")).toBe("false"));
    fireEvent.click(within(row("keyvault-site-icons")!).getByRole("switch"));
    await waitFor(() => expect(calls).toContainEqual(["settings.choose", { row: "keyvault-site-icons", option: "off" }]));
  });
});
