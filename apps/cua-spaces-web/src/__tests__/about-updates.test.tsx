// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Settings, About in the Electron app: a build with no update feed (a local
// build) keeps the SwiftUI app's update rows, greyed out, and says it can't
// update instead of hiding them; a build that updates draws them live; the
// version shows its build as the SwiftUI app's does.

import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { BridgeProvider } from "@/bridge";
import type { DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { ToastProvider } from "@/components/ui/toast";
import { TooltipProvider } from "@/components/ui/tooltip";
import { routeTree } from "../routeTree.gen";
import { stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

/** About on a host in `mode` whose `about.get` says `updater` and `build`. */
async function mount(mode: DataAdapter["mode"], updater: boolean) {
  const demo = createDemoAdapter({ latencyMs: 1, stepMs: 4 });
  const adapter: DataAdapter = {
    mode,
    subscribe: (l) => demo.subscribe(l),
    call: (async (op: string, args: never) => {
      const out = await (demo.call as (o: string, a: unknown) => Promise<unknown>)(op, args);
      return op === "about.get" ? { ...(out as object), updater, version: "0.7.2", build: "0.7.2.41" } : out;
    }) as DataAdapter["call"],
  };
  const router = createRouter({ routeTree, history: createMemoryHistory({ initialEntries: ["/settings/about"] }) });
  render(
    <BridgeProvider adapter={adapter} core={await testCore()} storeOptions={{ tickMs: 5 }}>
      <TooltipProvider delay={500}>
        <ToastProvider>
          <RouterProvider router={router} />
        </ToastProvider>
      </TooltipProvider>
    </BridgeProvider>,
  );
}

const q = (sel: string) => document.querySelector<HTMLElement>(sel);

describe.skipIf(!wasmBuilt)("Settings, About", () => {
  it("keeps the update rows in an Electron build that can't update, greyed out, and says so", async () => {
    await mount("electron", false);
    await waitFor(() => expect(q("[data-about-updates]")).not.toBeNull(), { timeout: 8000 });
    expect(q("[data-about-version]")!.textContent).toBe("Version 0.7.2 (0.7.2.41)");
    expect(q("[data-about-updates]")!.hasAttribute("data-about-unavailable")).toBe(true);
    expect(q("[data-about-last-check]")!.textContent).toBe("Updates aren’t available in this build");
    expect((q("[data-about-check]") as HTMLButtonElement).disabled).toBe(true);
    expect(q("[data-about-check]")!.textContent).toBe("Check Now");
    expect(q("[data-about-auto-check]")!.hasAttribute("data-disabled")).toBe(true);
    expect(q("[data-about-auto-install]")!.hasAttribute("data-disabled")).toBe(true);
    expect(document.body.textContent).toContain("Automatically check for updates");
    expect(document.body.textContent).toContain("Update to:");
  });

  it("draws them live where the app updates itself", async () => {
    await mount("electron", true);
    await waitFor(() => expect(q("[data-about-updates]")).not.toBeNull(), { timeout: 8000 });
    expect(q("[data-about-updates]")!.hasAttribute("data-about-unavailable")).toBe(false);
    expect((q("[data-about-check]") as HTMLButtonElement).disabled).toBe(false);
    expect(q("[data-about-last-check]")!.textContent).toMatch(/^Last check: /);
  });

  it("leaves them out on the SwiftUI host without an updater, as its own About does", async () => {
    await mount("webkit", false);
    await waitFor(() => expect(q("[data-about-version]")).not.toBeNull(), { timeout: 8000 });
    expect(q("[data-about-updates]")).toBeNull();
  });
});
