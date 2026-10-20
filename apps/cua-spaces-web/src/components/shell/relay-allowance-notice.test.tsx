// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import { createDemoAdapter } from "@/bridge/adapters/demo";
import { demoRelayUsage } from "@/bridge/adapters/demo/relay-usage";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { BridgeProvider, resetUpsellMemory, type CoreClient, type DataAdapter, type RelayUsage } from "@/bridge";

import { RelayAllowanceNotice } from "./relay-allowance-notice";

afterEach(() => {
  cleanup();
  localStorage.clear();
  resetUpsellMemory();
});

let core: CoreClient;
beforeAll(async () => {
  core = await testCore();
});

const warning: RelayUsage = { ...demoRelayUsage(), hoursUsed: 16.5, liveSessions: 0 };
const reached: RelayUsage = { ...demoRelayUsage(), hoursUsed: 20.2, softLimitReached: true, reasons: ["hours"] };

/** The demo host answering `usage`, recording the links it opens. */
function host(usage: RelayUsage | null, accountUrl?: string) {
  const demo = createDemoAdapter({ latencyMs: 0 });
  demo.state.relayUsage = usage;
  const opened: string[] = [];
  const adapter: DataAdapter = {
    ...demo,
    call: (op, args) => {
      if (op === "session.openExternal") opened.push((args as { url: string }).url);
      // The host names its account app (the page's core has no environment).
      if (op === "settings.get" && accountUrl) return demo.call(op, args).then((s) => ({ ...(s as object), accountUrl }) as typeof s);
      return demo.call(op, args);
    },
  };
  const view = () =>
    render(
      <BridgeProvider adapter={adapter} core={core}>
        <RelayAllowanceNotice />
      </BridgeProvider>,
    );
  return { demo, opened, view };
}

// The words are the app core's: these need the wasm build.
describe.skipIf(!wasmBuilt)("<RelayAllowanceNotice>", () => {
  it("shows nothing within the allowance", async () => {
    const { view } = host(demoRelayUsage());
    view();
    await act(() => new Promise((r) => setTimeout(r, 10)));
    expect(document.querySelector("[data-relay-allowance]")).toBeNull();
  });

  it("past the allowance: says the Spaces keep working, and Upgrade opens the plan's page", async () => {
    const { view, opened } = host(reached);
    view();
    const notice = await screen.findByRole("status");
    expect(notice.textContent).toContain("You've reached the free plan's remote limit for this month. Your Spaces keep working.");
    fireEvent.click(screen.getByRole("button", { name: "Upgrade" }));
    await waitFor(() => expect(opened).toEqual(["https://app.cua.ai/buy/pro?from=app_meter"]));
  });

  it("Upgrade opens the host's account app: Free to Pro, Pro to Teams", async () => {
    const staging = host({ ...reached, upgradeUrl: null }, "https://app.staging.cua.ai");
    staging.view();
    await waitFor(() => {
      fireEvent.click(screen.getByRole("button", { name: "Upgrade" }));
      expect(staging.opened).toContain("https://app.staging.cua.ai/buy/pro?from=app_meter");
    });
    cleanup();
    localStorage.clear();
    const pro = host({ ...reached, plan: "pro", upgradeUrl: null });
    pro.view();
    fireEvent.click(await screen.findByRole("button", { name: "Upgrade" }));
    await waitFor(() => expect(pro.opened).toEqual(["https://app.cua.ai/buy/teams?from=app_meter"]));
  });

  it("at 80% of the hours: a heads-up that says nothing stops, with Pro's checkout", async () => {
    const { view, opened } = host(warning);
    view();
    const notice = await screen.findByRole("status");
    expect(notice.getAttribute("data-relay-allowance")).toBe("warn");
    expect(notice.textContent).toContain("You've used 16.5 of 20 remote hours this month. Nothing stops at the limit.");
    fireEvent.click(screen.getByRole("button", { name: "Upgrade" }));
    await waitFor(() => expect(opened).toEqual(["https://app.cua.ai/buy/pro?from=app_meter"]));
  });

  it("the heads-up waits while a live session is going, but the limit notice does not", async () => {
    const busy = host({ ...warning, liveSessions: 1 });
    busy.view();
    await act(() => new Promise((r) => setTimeout(r, 20)));
    expect(document.querySelector("[data-relay-allowance]")).toBeNull();
    cleanup();
    host({ ...reached, liveSessions: 1 }).view();
    expect(await screen.findByRole("status")).toBeTruthy();
  });

  it("the heads-up shows once: one prompt every three days, and a closed one stays closed", async () => {
    host(warning).view();
    fireEvent.click(await screen.findByRole("button", { name: "Dismiss" }));
    cleanup();
    host(warning).view();
    await act(() => new Promise((r) => setTimeout(r, 20)));
    expect(document.querySelector("[data-relay-allowance]")).toBeNull();
    cleanup();
    // The limit notice has its own dismissal and is not held back by the heads-up's.
    host(reached).view();
    expect(await screen.findByRole("status")).toBeTruthy();
  });

  it("dismissed, it stays hidden for that month and reason, and shows again for another", async () => {
    const first = host(reached);
    first.view();
    fireEvent.click(await screen.findByRole("button", { name: "Dismiss" }));
    expect(document.querySelector("[data-relay-allowance]")).toBeNull();
    cleanup();

    host(reached).view();
    await act(() => new Promise((r) => setTimeout(r, 10)));
    expect(document.querySelector("[data-relay-allowance]")).toBeNull();
    cleanup();

    host({ ...reached, reasons: ["sessions", "hours"] }).view();
    expect(await screen.findByRole("status")).toBeTruthy();
  });
});
