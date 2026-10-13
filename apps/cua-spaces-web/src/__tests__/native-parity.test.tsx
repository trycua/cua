// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// What the New UI on a Mac must show as the SwiftUI app does (real routes, the wasm core, the demo host with the Mac's answers):
// Devices when this device is not enrolled, Settings, General as the core
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
  it("Devices: a device the relay has not enrolled gets one Enroll…, named as its system names it, as the SwiftUI page", async () => {
    await mount("/settings/devices", (op) =>
      op === "devices.get" ? Promise.resolve({ devices: [], audit: [], readError: NOT_ENROLLED, deviceName: "cua's Mac Studio" }) : undefined,
    );
    const status = await waitFor(() => {
      const s = document.querySelector<HTMLElement>("[data-this-device-status]");
      if (!s) throw new Error("no This Device row");
      return s;
    });
    expect(status.getAttribute("data-this-device-status")).toBe("needs-enrollment");
    expect(document.querySelector("[data-this-device-name]")?.textContent).toBe("cua's Mac Studio");
    // One Enroll…: the row's. The line under it says what it is for.
    expect(screen.getAllByRole("button", { name: /^Enroll/ })).toHaveLength(1);
    expect(document.querySelector("[data-banner-text]")?.textContent).toMatch(/Enroll this device/);
    // The relay's refusal is what "Needs enrollment" already says.
    expect(document.querySelector("[data-devices-read-error]")).toBeNull();
    expect(document.body.textContent).not.toContain("permission denied");
    expect(document.querySelector("[data-devices-error]")).toBeNull();
    fireEvent.click(document.querySelector("[data-this-device-enroll]")!);
    await waitFor(() => expect(screen.getByRole("dialog")).toBeTruthy());
  });

  it("Devices: another read failure still says why", async () => {
    await mount("/settings/devices", (op, _a, base) =>
      op === "devices.get" ? base(op, {}).then((d) => ({ ...(d as object), readError: "relay: timed out" })) : undefined,
    );
    await waitFor(() => expect(document.querySelector("[data-devices-read-error]")?.textContent).toBe("relay: timed out"));
  });

  it("General is the core's page: Teams, Welcome, the notch's words and the privacy line with its link", async () => {
    const { router, calls } = await mount("/settings");
    const account = await waitFor(() => {
      const r = document.querySelector<HTMLElement>('[data-setting-row="teams"]');
      if (!r) throw new Error("no Teams row");
      return r;
    });
    expect(account.textContent).toContain("Coming soon");
    expect(within(account).getByText("Join the waitlist")).toBeTruthy();
    const note = document.querySelector<HTMLElement>('[data-setting-row="telemetry-note"]')!;
    expect(note.textContent).toMatch(/^Features used, sandbox types, durations and error categories/);
    expect(within(note).getByText("What is collected")).toBeTruthy();
    expect(document.body.textContent).not.toContain("Crash reports and feature counts");
    // The Teams waitlist opens the website.
    fireEvent.click(within(account).getByText("Join the waitlist"));
    await waitFor(() => expect(calls.some(([op, a]) => op === "session.openExternal" && String((a as { url: string }).url).includes("cua.ai"))).toBe(true));
    // Welcome's Show again starts the first run over (this UI's, off the Mac).
    const welcome = document.querySelector<HTMLElement>('[data-setting-row="welcome"]')!;
    fireEvent.click(within(welcome).getByText("Show again"));
    await waitFor(() => expect(router.state.location.pathname).toBe("/onboarding"));
  });

  it("the runtime choices never wrap", async () => {
    await mount("/settings");
    await waitFor(() => expect(document.querySelector('[data-setting-row="telemetry"]')).toBeTruthy());
    const toggle = document.querySelector<HTMLElement>('[data-setting-row="telemetry"] button');
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
