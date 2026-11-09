// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The title bar's environment tag: "Local · Staging" or "Local · Prod" on a
// Local build, nothing on the released app on production. The host names the
// channel and environment (`settings.get`'s `target`); the app core decides
// what shows (`environment.tag`).

import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { BridgeProvider } from "@/bridge";
import type { DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import type { AppTarget, SettingsSnapshot, UpdateChannel } from "@/bridge/protocol";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { ToastProvider } from "@/components/ui/toast";
import { TooltipProvider } from "@/components/ui/tooltip";
import { routeTree } from "../routeTree.gen";
import { stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

/** The app on a host whose `settings.get` names `target` (null: none) and `updateChannel`. */
async function mount(target: AppTarget | null, updateChannel: UpdateChannel | null = null) {
  settingsRead = false;
  const demo = createDemoAdapter({ latencyMs: 1, stepMs: 4 });
  const adapter: DataAdapter = {
    mode: "webkit",
    subscribe: (l) => demo.subscribe(l),
    call: (async (op: string, args: never) => {
      const out = await (demo.call as (o: string, a: unknown) => Promise<unknown>)(op, args);
      if (op !== "settings.get") return out;
      const s = out as SettingsSnapshot;
      settingsRead = true;
      return { ...s, target, values: { ...s.values, updateChannel } };
    }) as DataAdapter["call"],
  };
  const router = createRouter({ routeTree, history: createMemoryHistory({ initialEntries: ["/spaces"] }) });
  render(
    <BridgeProvider adapter={adapter} core={await testCore()} storeOptions={{ tickMs: 5 }}>
      <TooltipProvider delay={500}>
        <ToastProvider>
          <RouterProvider router={router} />
        </ToastProvider>
      </TooltipProvider>
    </BridgeProvider>,
  );
  // The page is drawn once its title is.
  await waitFor(() => expect(document.querySelector("header")?.textContent).toContain("Computers"), { timeout: 8000 });
}

let settingsRead = false;

/** The page has read the host's settings (and drawn from them). */
async function settled() {
  await waitFor(() => expect(settingsRead).toBe(true));
  await new Promise((r) => setTimeout(r, 50));
}

const tag = () => document.querySelector<HTMLElement>("[data-environment-tag]");

describe.skipIf(!wasmBuilt)("the environment tag", () => {
  it("says Local · Staging on a Local build on staging, with the sign-in server", async () => {
    await mount({ channel: "local", env: "staging", authHost: "auth.staging.cua.ai" });
    const note = await screen.findByRole("note", { name: "This app uses Cua Staging (auth.staging.cua.ai)" });
    expect(note.textContent).toBe("Local · Staging");
    expect(note.dataset.environmentTag).toBe("staging");
    expect(note.closest("header")).not.toBeNull();
  });

  it("says Local · Prod on a Local build that opted in to production", async () => {
    await mount({ channel: "local", env: "prod", authHost: "auth.cua.ai" });
    const note = await screen.findByRole("note", { name: "This app uses Cua Production (auth.cua.ai)" });
    expect(note.textContent).toBe("Local · Prod");
    expect(note.dataset.environmentTag).toBe("prod");
  });

  it("shows nothing on the released app on production, Nightly updates included", async () => {
    for (const updates of ["stable", "nightly"] as const) {
      await mount({ channel: "stable", env: "prod", authHost: "auth.cua.ai" }, updates);
      await settled();
      expect(tag()).toBeNull();
      cleanup();
    }
  });

  it("names Nightly on a Nightly build pointed at staging", async () => {
    await mount({ channel: "stable", env: "staging", authHost: "" }, "nightly");
    const note = await screen.findByRole("note", { name: "This app uses Cua Staging (auth.staging.cua.ai)" });
    expect(note.textContent).toBe("Nightly · Staging");
  });

  it("shows nothing when the host can't say", async () => {
    await mount(null);
    await settled();
    expect(tag()).toBeNull();
  });
});
