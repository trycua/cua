// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The smaller gaps the Electron dogfood found next to the SwiftUI app:
// no "New UI (preview)" switch in Electron, New Space's "Show images"
// button, the detail's power button in the Swift app's words, and a
// desktop playing in picture in picture said in place instead of twice.

import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { BridgeProvider } from "@/bridge";
import type { DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { resetNewSpaceSession } from "@/bridge/new-space";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { ToastProvider } from "@/components/ui/toast";
import { TooltipProvider } from "@/components/ui/tooltip";
import { routeTree } from "../routeTree.gen";
import { stubBrowserApis } from "./app-harness";

// The wizard's open state is the page's session: one test's New Space must not stay open in the next.
afterEach(() => {
  cleanup();
  resetNewSpaceSession();
});
stubBrowserApis();

async function mount(path: string, mode: DataAdapter["mode"] = "demo") {
  const demo = createDemoAdapter({ latencyMs: 1, stepMs: 4 });
  const adapter: DataAdapter = { mode, subscribe: (l) => demo.subscribe(l), call: demo.call };
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
  return { demo, router };
}

const q = (sel: string) => document.querySelector<HTMLElement>(sel);

describe.skipIf(!wasmBuilt)("Electron parity, the small things", () => {
  it("has no New UI (preview) switch in Electron (it is that UI); the SwiftUI host keeps it", { timeout: 20_000 }, async () => {
    await mount("/settings/experiments", "electron");
    await waitFor(() => expect(q("[data-settings-section]")).not.toBeNull(), { timeout: 8000 });
    expect(q('[data-setting-row="experiment:web_ui"]')).toBeNull();
    expect(document.querySelectorAll("[data-setting-row^='experiment:']").length).toBeGreaterThan(0);
    cleanup();
    await mount("/settings/experiments", "demo");
    await waitFor(() => expect(q('[data-setting-row="experiment:web_ui"]')).not.toBeNull(), { timeout: 8000 });
  });

  it("opens and closes New Space's image list from its Show images button", { timeout: 20_000 }, async () => {
    await mount("/spaces");
    fireEvent.click(await screen.findByRole("button", { name: /New Space/ }, { timeout: 8000 }));
    const toggle = await screen.findByRole("button", { name: "Show images" }, { timeout: 8000 });
    expect(q("[data-image-suggestions]")).toBeNull();
    fireEvent.click(toggle);
    await waitFor(() => expect(q("[data-image-suggestions]")).not.toBeNull());
    fireEvent.click(toggle);
    await waitFor(() => expect(q("[data-image-suggestions]")).toBeNull());
  });

  it("words the detail's power button as the SwiftUI app does", { timeout: 20_000 }, async () => {
    await mount("/spaces/relay%3Amac-mini%2Fqa-windows");
    expect(await screen.findByRole("button", { name: "Turn off" }, { timeout: 8000 })).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Turn off" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Turn off qa-windows?")).toBeTruthy();
    cleanup();
    await mount("/spaces/local%3Adesign-review");
    expect(await screen.findByRole("button", { name: "Suspend" }, { timeout: 8000 })).toBeTruthy();
  });

  it("says the desktop plays in a floating window, with Bring it back, instead of streaming it twice", { timeout: 20_000 }, async () => {
    const { demo } = await mount("/spaces/local%3Adesign-review");
    await waitFor(() => expect(q('[data-pip="desktop"]')).not.toBeNull(), { timeout: 8000 });
    await act(async () => {
      fireEvent.click(q('[data-pip="desktop"]')!);
      await new Promise((r) => setTimeout(r, 50));
    });
    await waitFor(() => expect(q("[data-popped-out]")).not.toBeNull());
    expect(within(q("[data-popped-out]")!).getByText("Playing in a floating window")).toBeTruthy();
    fireEvent.click(within(q("[data-popped-out]")!).getByRole("button", { name: "Bring it back" }));
    await waitFor(() => expect(q("[data-popped-out]")).toBeNull());
    expect((await demo.call("spaces.windows", { spaceId: "local:design-review" })).open).toEqual([]);
  });
});
