// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// What the New UI on a Mac must show as the SwiftUI app does (real routes, the wasm core, the demo host with the Mac's answers):
// This machine's trust when it is not trusted yet, Settings, General as the core
// lays it out, and the Keyvault's reason when it is unavailable.

import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
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

type Answer = (op: string, args: Record<string, unknown>, demo: (op: string, args: unknown) => Promise<unknown>) => Promise<unknown> | undefined;

/** The app at `path` on the demo host, with some answers replaced. */
async function mount(path: string, answer: Answer = () => undefined) {
  const demo = createDemoAdapter({ latencyMs: 1, stepMs: 4 });
  const calls: [string, unknown][] = [];
  const base = (op: string, args: unknown) => (demo.call as (o: string, a: unknown) => Promise<unknown>)(op, args);
  const adapter: DataAdapter = {
    mode: demo.mode,
    subscribe: (l) => demo.subscribe(l),
    call: (async (op: string, args: Record<string, unknown>) => {
      calls.push([op, args]);
      return (await answer(op, args, base)) ?? base(op, args);
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

const NOT_ENROLLED =
  "permission denied: relay: this device is not enrolled for your cua.ai account: sign in again (`cua auth login`) to enroll it, run `cua devices enroll`, or approve it from the Cua Spaces app on an enrolled device";

describe.skipIf(!wasmBuilt)("as the SwiftUI app (real routes, wasm core)", () => {
  it("Machines: a machine the relay has not trusted gets one Trust this machine, on its own page", async () => {
    await mount("/machines", (op) =>
      op === "devices.get" ? Promise.resolve({ devices: [], audit: [], readError: NOT_ENROLLED, deviceName: "cua's Mac Studio" }) : undefined,
    );
    const toggle = await waitFor(() => {
      const t = document.querySelector<HTMLElement>("[data-machine-details-toggle]");
      if (!t) throw new Error("no Details on this machine's page");
      return t;
    });
    fireEvent.click(toggle);
    const status = await waitFor(() => {
      const s = document.querySelector<HTMLElement>("[data-machine-trust]");
      if (!s) throw new Error("no trust section on this machine's page");
      return s;
    });
    expect(status.getAttribute("data-machine-trust")).toBe("needs-enrollment");
    // One button; the line under it says what it is for.
    expect(screen.getAllByRole("button", { name: /^Trust this machine/ })).toHaveLength(1);
    expect(document.querySelector("[data-machine-trust-text]")?.textContent).toMatch(/Trust this machine/);
    // The core's words are plain: no enrolling, no devices.
    expect(document.body.textContent).not.toMatch(/enroll|\bdevices?\b/i);
    // The relay's refusal is what "Not trusted yet" already says.
    expect(document.querySelector("[data-devices-read-error]")).toBeNull();
    expect(document.body.textContent).not.toContain("permission denied");
    fireEvent.click(document.querySelector("[data-machine-trust-action]")!);
    await waitFor(() => expect(screen.getByRole("dialog")).toBeTruthy());
  });

  it("Machines: another read failure still says why", async () => {
    await mount("/machines", (op, _a, base) =>
      op === "devices.get" ? base(op, {}).then((d) => ({ ...(d as object), readError: "relay: timed out" })) : undefined,
    );
    await waitFor(() => expect(document.querySelector("[data-devices-read-error]")?.textContent).toBe("relay: timed out"));
  });

  it("Account is the core's page: the plan, the team, and one button for the web account", async () => {
    const { calls } = await mount("/settings");
    const manage = await waitFor(() => {
      const r = document.querySelector<HTMLElement>("[data-manage-account]");
      if (!r) throw new Error("no Manage button");
      return r;
    });
    // The real Teams section: no waitlist, and invites, roles and billing are the account app's.
    expect(document.body.textContent).not.toContain("Coming soon");
    expect(document.body.textContent).not.toContain("waitlist");
    expect(manage.textContent).toBe("Manage account on the web");
    fireEvent.click(manage);
    await waitFor(() =>
      expect(calls.some(([op, a]) => op === "session.openExternal" && (a as { url: string }).url === "https://app.cua.ai/team")).toBe(true),
    );
  });

  it("General keeps the privacy line with its link; Welcome's Show again is in About", async () => {
    const { router } = await mount("/settings/general");
    const row = await waitFor(() => {
      const r = document.querySelector<HTMLElement>('[data-setting-row="telemetry"]');
      if (!r) throw new Error("no usage data row");
      return r;
    });
    expect(row.textContent).toMatch(/Which features you use and when something goes wrong\./);
    expect(within(document.querySelector<HTMLElement>("[data-telemetry-link]")!.parentElement!).getByText("What is collected")).toBeTruthy();
    expect(document.body.textContent).not.toContain("Crash reports and feature counts");
    await router.navigate({ to: "/settings/about" });
    const welcome = await waitFor(() => {
      const w = document.querySelector<HTMLElement>("[data-about-welcome]");
      if (!w) throw new Error("no welcome button");
      return w;
    });
    // Show starts the first run over (this UI's, off the Mac).
    fireEvent.click(welcome);
    await waitFor(() => expect(router.state.location.pathname).toBe("/onboarding"));
  });

  it("the choices never wrap", async () => {
    await mount("/settings/general");
    await waitFor(() => expect(document.querySelector('[aria-label="Appearance"]')).toBeTruthy());
    const toggle = document.querySelector<HTMLElement>('[aria-label="Appearance"] button');
    expect(toggle?.className).toContain("whitespace-nowrap");
  });

  it("Keyvault: unavailable says why and what to do, as the SwiftUI page does", async () => {
    const why =
      "The process serving the Keyvault (com.trycua.cua (ad hoc signature, UNVERIFIED)) is not signed by Cua, so Cua will not talk to it and the Keyvault is off.";
    await mount("/keyvault", (op, _a, base) =>
      op === "keyvault.overview"
        ? base(op, {}).then((o) => ({ ...(o as object), availability: "impostor", message: why, items: [], itemsTotal: 0 }))
        : undefined,
    );
    const notice = await waitFor(() => {
      const n = document.querySelector<HTMLElement>("[data-vault-notice]");
      if (!n) throw new Error("no notice");
      return n;
    });
    expect(notice.textContent).toMatch(/^Keyvault is unavailable: the Cua daemon is not signed by Cua/);
    expect(document.querySelector("[data-vault-notice-detail]")?.textContent).toBe(why);
  });
});
