// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import { createDemoAdapter } from "@/bridge/adapters/demo";
import { demoRelayUsage } from "@/bridge/adapters/demo/relay-usage";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import { BridgeProvider, flagOn, resetUpsellMemory, setFlagOverride, setRemoteFlags, type CoreClient, type DataAdapter, type RelayUsage, type UpsellCardReason } from "@/bridge";

import { RelayAllowanceNotice } from "./shell/relay-allowance-notice";
import { UpgradeCard } from "./upgrade-card";

afterEach(() => {
  cleanup();
  localStorage.clear();
  setRemoteFlags({});
  resetUpsellMemory();
});

let core: CoreClient;
beforeAll(async () => {
  core = await testCore();
});

const reached: RelayUsage = { ...demoRelayUsage(), hoursUsed: 20.2, softLimitReached: true, reasons: ["hours"], liveSessions: 0 };

function host(usage: RelayUsage | null) {
  const demo = createDemoAdapter({ latencyMs: 0 });
  demo.state.relayUsage = usage;
  const opened: string[] = [];
  const features: string[] = [];
  const adapter: DataAdapter = {
    ...demo,
    call: (op, args) => {
      if (op === "session.openExternal") opened.push((args as { url: string }).url);
      if (op === "telemetry.track") for (const s of (args as { signals: { feature?: string }[] }).signals) if (s.feature) features.push(s.feature);
      return demo.call(op, args);
    },
  };
  const view = (reason?: UpsellCardReason) =>
    render(
      <BridgeProvider adapter={adapter} core={core}>
        {reason ? <UpgradeCard reason={reason} /> : <RelayAllowanceNotice />}
      </BridgeProvider>,
    );
  return { opened, features, view };
}

describe("the upsell_cards flag", () => {
  it("is off by default, and a device override or a remote value turns it on", () => {
    expect(flagOn("upsell_cards")).toBe(false);
    setRemoteFlags({ upsell_cards: true });
    expect(flagOn("upsell_cards")).toBe(true);
    setFlagOverride("upsell_cards", false);
    expect(flagOn("upsell_cards")).toBe(false);
    setFlagOverride("upsell_cards", null);
    expect(flagOn("upsell_cards")).toBe(true);
  });
});

describe.skipIf(!wasmBuilt)("<UpgradeCard>", () => {
  it("draws nothing while the flag is off", async () => {
    const { view } = host(reached);
    view("hours");
    await act(() => new Promise((r) => setTimeout(r, 20)));
    expect(document.querySelector("[data-upgrade-card]")).toBeNull();
  });

  it("flag on: says what happened, what Pro gives and the price; Upgrade opens the Pro checkout and is counted", async () => {
    setFlagOverride("upsell_cards", true);
    const { view, opened, features } = host(reached);
    view("hours");
    const card = await waitFor(() => {
      const el = document.querySelector("[data-upgrade-card]");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(card.getAttribute("data-upgrade-card")).toBe("hours");
    expect(card.textContent).toContain("You've used this month's remote hours");
    expect(card.textContent).toContain("Pro gives you 200 hours and 5 live sessions");
    expect(card.textContent).toContain("$18 a month");
    fireEvent.click(screen.getByRole("button", { name: "Upgrade to Pro" }));
    await waitFor(() => expect(opened).toEqual(["https://app.cua.ai/buy/pro?from=app_card_hours"]));
    await waitFor(() => expect(features).toEqual(["upsell_card_hours_shown", "upsell_card_hours_click"]));
  });

  it("Not now closes it and is counted", async () => {
    setFlagOverride("upsell_cards", true);
    const { view, features } = host(reached);
    view("invite");
    fireEvent.click(await screen.findByRole("button", { name: "Not now" }));
    await waitFor(() => expect(document.querySelector("[data-upgrade-card]")).toBeNull());
    await waitFor(() => expect(features).toEqual(["upsell_card_invite_shown", "upsell_card_invite_dismiss"]));
  });

  it("a team plan has nothing to buy for an invite", async () => {
    setFlagOverride("upsell_cards", true);
    const { view } = host({ ...reached, plan: "team" });
    view("invite");
    await act(() => new Promise((r) => setTimeout(r, 20)));
    expect(document.querySelector("[data-upgrade-card]")).toBeNull();
  });
});

describe.skipIf(!wasmBuilt)("<RelayAllowanceNotice> with the cards on", () => {
  it("a used allowance is a card, and its Not now keeps it closed for the month", async () => {
    setFlagOverride("upsell_cards", true);
    const { view } = host(reached);
    view();
    const card = await waitFor(() => {
      const el = document.querySelector("[data-upgrade-card]");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(card.textContent).toContain("Pro gives you 200 hours");
    expect(document.querySelector("[data-relay-allowance='limit']")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Not now" }));
    await waitFor(() => expect(document.querySelector("[data-upgrade-card]")).toBeNull());
  });

  it("a Pro account at 5 of 5 live sessions with empty reasons gets the sessions card", async () => {
    setFlagOverride("upsell_cards", true);
    const { view } = host({ ...reached, plan: "pro", hoursUsed: 10, hoursLimit: 200, liveSessions: 5, liveSessionsLimit: 5, reasons: [], softLimitReached: false });
    view();
    const card = await waitFor(() => {
      const el = document.querySelector("[data-upgrade-card]");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(card.getAttribute("data-upgrade-card")).toBe("sessions");
    expect(card.textContent).toContain("You're using 5 of 5 live sessions");
  });

  it("a plan with no card keeps the notice and its Upgrade link", async () => {
    setFlagOverride("upsell_cards", true);
    const { view } = host({ ...reached, plan: "enterprise" });
    view();
    await screen.findByRole("status");
    expect(document.querySelector("[data-upgrade-card]")).toBeNull();
  });

  it("with the flag off the thin line is unchanged", async () => {
    const { view } = host(reached);
    view();
    await screen.findByRole("status");
    expect(document.querySelector("[data-upgrade-card]")).toBeNull();
  });
});
