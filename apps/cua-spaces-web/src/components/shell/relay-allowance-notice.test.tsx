// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import { createDemoAdapter } from "@/bridge/adapters/demo";
import { demoRelayUsage } from "@/bridge/adapters/demo/relay-usage";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { BridgeProvider, type CoreClient, type DataAdapter, type RelayUsage } from "@/bridge";

import { RelayAllowanceNotice } from "./relay-allowance-notice";

afterEach(() => {
  cleanup();
  localStorage.clear();
});

let core: CoreClient;
beforeAll(async () => {
  core = await testCore();
});

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
    expect(notice.textContent).toContain("You've used 20.2 of your 20 free relay hours this month. Your Spaces keep working. Upgrade for more.");
    fireEvent.click(screen.getByRole("button", { name: "Upgrade" }));
    // A free account goes to Pro's checkout, not the pricing page.
    await waitFor(() => expect(opened).toEqual(["https://app.cua.ai/buy/pro"]));
  });

  it("Pro goes to the Teams checkout", async () => {
    const { view, opened } = host({ ...reached, plan: "pro" });
    view();
    fireEvent.click(await screen.findByRole("button", { name: "Upgrade" }));
    await waitFor(() => expect(opened).toEqual(["https://app.cua.ai/buy/teams"]));
  });

  it("at 80% of the free hours it warns early, with the usage, and blocks nothing", async () => {
    const { view } = host({ ...demoRelayUsage(), hoursUsed: 16.5 });
    view();
    const notice = await screen.findByRole("status");
    expect(notice.textContent).toContain("You've used 16.5 of your 20 free relay hours this month. Your Spaces keep working, even past the limit.");
  });

  it("says sessions apart from hours", async () => {
    const { view } = host({ ...demoRelayUsage(), liveSessions: 3, softLimitReached: true, reasons: ["sessions"] });
    view();
    const notice = await screen.findByRole("status");
    expect(notice.textContent).toContain("You have 3 live sessions open. Your free plan includes 2 at once.");
    expect(notice.textContent).not.toContain("relay hours");
  });

  it("with no plan named and no page from the relay, Upgrade opens the host's account app billing page", async () => {
    const staging = host({ ...reached, plan: "", upgradeUrl: null }, "https://app.staging.cua.ai");
    staging.view();
    await waitFor(() => {
      fireEvent.click(screen.getByRole("button", { name: "Upgrade" }));
      expect(staging.opened).toContain("https://app.staging.cua.ai/billing");
    });
    cleanup();
    // A host that names none: the production account app.
    const prod = host({ ...reached, plan: "", upgradeUrl: null });
    prod.view();
    fireEvent.click(await screen.findByRole("button", { name: "Upgrade" }));
    await waitFor(() => expect(prod.opened).toEqual(["https://app.cua.ai/billing"]));
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
