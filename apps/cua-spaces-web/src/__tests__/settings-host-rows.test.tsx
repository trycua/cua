// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings on a host with settings of its own (the SwiftUI app): Runtimes,
// "Connect to the desktop automatically" and the Keyvault's auto-wipe, as
// the core lays them out, changed with `settings.choose`. Elsewhere the
// rows are left out.

import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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
  const ROW: Record<string, keyof HostSettings> = { "macos-runtime": "lumeSource", "linux-runtime": "linuxSource", "auto-connect": "autoConnect", "placement-overflow": "overflow", "keyvault-auto-wipe": "keyvaultAutoWipe" };
  const adapter: DataAdapter = {
    mode: demo.mode,
    subscribe: (l) => demo.subscribe(l),
    call: (async (op: string, args: { row: string; option: string }) => {
      if (op === "settings.choose") {
        chosen.push([args.row, args.option]);
        const key = ROW[args.row]!;
        (settings as Record<string, unknown>)[key] = key === "autoConnect" || key === "overflow" || key === "keyvaultAutoWipe" ? args.option === "on" : args.option;
        op = "settings.get";
      }
      const out = await demo.call(op as never, args as never);
      return op === "settings.get" && own ? { ...(out as object), hostSettings: { ...settings } } : out;
    }) as DataAdapter["call"],
  };
  const router = createRouter({ routeTree, history: createMemoryHistory({ initialEntries: ["/settings/advanced"] }) });
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
  it("lays out which software runs Spaces and auto-connect in Advanced, in plain words, and picks their options", async () => {
    const { chosen } = await mountSettings({ lumeSource: "system", linuxSource: "auto", autoConnect: true, overflow: true, keyvaultAutoWipe: false });
    const macos = await waitFor(() => {
      const r = row("macos-runtime");
      if (!r) throw new Error("no runtime row");
      return r;
    });
    expect(within(macos).getByText("Mac computers")).toBeTruthy();
    expect(macos.textContent).toContain("Already installed");
    expect(document.body.textContent).toContain("Which software runs Spaces on this Mac");
    expect(document.body.textContent).not.toMatch(/Lume|gVisor|Runtimes/);
    expect(within(row("linux-runtime")!).getByText("Linux computers")).toBeTruthy();
    expect(within(row("auto-connect")!).getByText("Open a Space\u2019s screen right away")).toBeTruthy();
    // Overflow into your cloud, which the SwiftUI host reads too.
    expect(within(row("placement-overflow")!).getByText("Use my own cloud when this Mac is busy")).toBeTruthy();
    // The Keyvault's switches live on the Keyvault page now.
    expect(row("keyvault-auto-wipe")).toBeNull();

    fireEvent.click(within(row("auto-connect")!).getByRole("switch"));
    await waitFor(() => expect(chosen).toContainEqual(["auto-connect", "off"]));
    const trigger = within(macos).getByRole("combobox");
    trigger.focus();
    fireEvent.keyDown(trigger, { key: "ArrowDown" });
    const option = await screen.findByRole("option", { name: "Cua\u2019s own" });
    fireEvent.pointerMove(option, { pointerType: "mouse" });
    fireEvent.mouseMove(option);
    fireEvent.keyDown(option, { key: "Enter" });
    fireEvent.keyUp(option, { key: "Enter" });
    await waitFor(() => expect(chosen).toContainEqual(["macos-runtime", "builtin"]));
  });

  it("leaves the rows out on a host without them", async () => {
    await mountSettings(null);
    await waitFor(() => expect(document.querySelector("[data-setting-row]")).toBeTruthy());
    expect(row("macos-runtime")).toBeNull();
    expect(row("auto-connect")).toBeNull();
    expect(row("placement-overflow")).toBeNull();
    expect(row("keyvault-auto-wipe")).toBeNull();
  });
});
