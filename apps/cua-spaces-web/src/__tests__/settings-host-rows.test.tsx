// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings on a host with settings of its own (the SwiftUI app): Runtimes,
// "Connect to the desktop automatically" and the Keyvault's auto-wipe, as
// the core lays them out, changed with `settings.choose`. Elsewhere the
// rows are left out.

import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { cleanup, fireEvent, render, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { BridgeProvider, type HostSettings } from "@/bridge";
import type { DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { ToastProvider } from "@/components/ui/toast";
import { TooltipProvider } from "@/components/ui/tooltip";
import { routeTree } from "../routeTree.gen";
import { stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

/** The demo host, with these host settings (none: the demo's own answer). */
async function mountSettings(own: HostSettings | null) {
  const demo = createDemoAdapter({ latencyMs: 1, stepMs: 4 });
  const chosen: [string, string][] = [];
  const settings = { ...own };
  const ROW: Record<string, keyof HostSettings> = { "macos-runtime": "lumeSource", "linux-runtime": "linuxSource", "auto-connect": "autoConnect", "keyvault-auto-wipe": "keyvaultAutoWipe" };
  const adapter: DataAdapter = {
    mode: demo.mode,
    subscribe: (l) => demo.subscribe(l),
    call: (async (op: string, args: { row: string; option: string }) => {
      if (op === "settings.choose") {
        chosen.push([args.row, args.option]);
        const key = ROW[args.row]!;
        (settings as Record<string, unknown>)[key] = key === "autoConnect" || key === "keyvaultAutoWipe" ? args.option === "on" : args.option;
        op = "settings.get";
      }
      const out = await demo.call(op as never, args as never);
      return op === "settings.get" && own ? { ...(out as object), hostSettings: { ...settings } } : out;
    }) as DataAdapter["call"],
  };
  const router = createRouter({ routeTree, history: createMemoryHistory({ initialEntries: ["/settings"] }) });
  render(
    <BridgeProvider adapter={adapter} core={await testCore()} storeOptions={{ tickMs: 5 }}>
      <TooltipProvider delay={500}>
        <ToastProvider>
          <RouterProvider router={router} />
        </ToastProvider>
      </TooltipProvider>
    </BridgeProvider>,
  );
  return { chosen };
}

const row = (id: string) => document.querySelector<HTMLElement>(`[data-setting-row="${id}"]`);

describe.skipIf(!wasmBuilt)("the host's own settings (real routes, wasm core)", () => {
  it("lays out Runtimes, auto-connect and the Keyvault's auto-wipe, and picks their options", async () => {
    const { chosen } = await mountSettings({ lumeSource: "system", linuxSource: "auto", autoConnect: true, keyvaultAutoWipe: false });
    const runtimes = await waitFor(() => {
      const s = document.querySelector<HTMLElement>('[data-settings-section="runtimes"]');
      if (!s) throw new Error("no Runtimes");
      return s;
    });
    expect(within(runtimes).getByText("Runtimes")).toBeTruthy();
    const macos = row("macos-runtime")!;
    expect(within(macos).getByText("macOS VMs")).toBeTruthy();
    // The note under it is the core's.
    expect(macos.textContent).toMatch(/Built-in is Cua’s signed Lume/);
    expect(within(row("linux-runtime")!).getByText("Linux")).toBeTruthy();
    expect(within(row("auto-connect")!).getByText("Connect to the desktop automatically")).toBeTruthy();
    expect(within(row("keyvault-auto-wipe")!).getByText("Wipe access from Spaces automatically")).toBeTruthy();
    // Only the auto-wipe rows of the Keyvault section.
    expect(row("keyvault-site-icons")).toBeNull();

    fireEvent.click(within(macos).getByText("Built-in"));
    await waitFor(() => expect(chosen).toEqual([["macos-runtime", "builtin"]]));
    fireEvent.click(within(row("auto-connect")!).getByRole("switch"));
    await waitFor(() => expect(chosen).toContainEqual(["auto-connect", "off"]));
    fireEvent.click(within(row("keyvault-auto-wipe")!).getByRole("switch"));
    await waitFor(() => expect(chosen).toContainEqual(["keyvault-auto-wipe", "on"]));
  });

  it("leaves the rows out on a host without them", async () => {
    await mountSettings(null);
    await waitFor(() => expect(document.querySelector("[data-setting-row]")).toBeTruthy());
    expect(document.querySelector('[data-settings-section="runtimes"]')).toBeNull();
    expect(row("auto-connect")).toBeNull();
    expect(row("keyvault-auto-wipe")).toBeNull();
  });
});
