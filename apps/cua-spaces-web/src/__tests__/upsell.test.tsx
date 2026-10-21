// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The in-app Pro and Teams prompts, on the real routes and the wasm core:
// the plan card in Settings and the plan under the account name (always
// there), the second-machine prompt on Machines, and the share prompt. The
// rules (one prompt every 3 days, never during a live session, closable) are
// the core's `upsell`.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import { createDemoAdapter } from "@/bridge/adapters/demo";
import { demoRelayUsage } from "@/bridge/adapters/demo/relay-usage";
import { testCore, wasmBuilt } from "@/bridge/__tests__/testCore";
import type { MachineRow } from "@/bridge/contracts/host";
import { BridgeProvider, resetUpsellMemory, type CoreClient, type DataAdapter, type RelayUsage } from "@/bridge";
import { UpsellPrompt } from "@/components/upsell-prompt";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

afterEach(() => {
  cleanup();
  localStorage.clear();
  resetUpsellMemory();
});
stubBrowserApis();

const idle = (over: Partial<RelayUsage> = {}) => ({ ...demoRelayUsage(), liveSessions: 0, ...over });

function host(usage: RelayUsage, opened: string[] = [], tracked: unknown[] = [], runs = false): TestHost {
  return {
    failsCreates: false,
    listsGhost: false,
    answer: (op, out) => {
      if (op === "relay.usage") return usage;
      // The demo host has an agent run going; these tests start with none.
      if (op === "agents.runs" && !runs) return [];
      if (op === "machines.list") {
        return [...(out as MachineRow[]), { id: "studio", name: "Studio", via: "relay", online: true, os: "", detail: "", limits: [] }];
      }
      return out;
    },
    intercept: (op, args) => {
      if (op === "session.openExternal") {
        opened.push((args as { url: string }).url);
        return null;
      }
      if (op === "telemetry.track") {
        tracked.push(...(args as { signals: unknown[] }).signals);
        return null;
      }
      return undefined;
    },
  };
}

const features = (tracked: unknown[]) => tracked.map((s) => (s as { feature?: string }).feature).filter(Boolean);

describe.skipIf(!wasmBuilt)("in-app Pro and Teams prompts (real routes, wasm core)", () => {
  it("Settings always shows the plan card: Free offers Pro and a team, each to its checkout", async () => {
    const opened: string[] = [];
    const tracked: unknown[] = [];
    const { router } = await mountApp("/settings", host(idle({ liveSessions: 1 }), opened, tracked));
    await router.load();
    const card = await waitFor(() => {
      const el = document.querySelector("[data-plan-card]");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(card.getAttribute("data-plan-card")).toBe("free");
    expect(card.textContent).toContain("Free plan");
    expect(card.textContent).toContain("2 live sessions and 20 remote hours a month");
    fireEvent.click(card.querySelector("[data-plan-primary]")!);
    fireEvent.click(card.querySelector("[data-plan-secondary]")!);
    await waitFor(() => expect(opened).toEqual(["https://app.cua.ai/buy/pro?from=app_plan_card", "https://app.cua.ai/buy/teams?from=app_plan_card"]));
    await waitFor(() => expect(features(tracked)).toContain("upsell_plan_card_click"));
  });

  it("Settings shows the usage meter: Pro at 5 of 5 live sessions says so, with Upgrade to Teams", async () => {
    const opened: string[] = [];
    const { router } = await mountApp("/settings", host(idle({ plan: "pro", liveSessions: 5, liveSessionsLimit: 5, hoursUsed: 12, hoursLimit: 200 }), opened));
    await router.load();
    const sessions = await waitFor(() => {
      const el = document.querySelector('[data-usage-meter="Live sessions"]');
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(sessions.getAttribute("aria-valuenow")).toBe("5");
    expect(sessions.getAttribute("aria-valuemax")).toBe("5");
    expect(sessions.hasAttribute("data-full")).toBe(true);
    const hours = document.querySelector('[data-usage-meter="Remote hours"]')!;
    expect(hours.getAttribute("aria-valuenow")).toBe("12");
    expect(hours.hasAttribute("data-full")).toBe(false);
    const notice = await waitFor(() => {
      const el = document.querySelector("[data-relay-allowance]");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(notice.textContent).toContain("You're using 5 of 5 live sessions on Pro. Close one or upgrade to Teams.");
    fireEvent.click(notice.querySelector("[data-relay-upgrade]")!);
    await waitFor(() => expect(opened).toEqual(["https://app.cua.ai/buy/teams?from=app_meter"]));
  });

  it("Settings shows Pro's plan card with Manage plan and Teams", async () => {
    const { router } = await mountApp("/settings", host(idle({ plan: "pro", liveSessionsLimit: 5, hoursLimit: 200 })));
    await router.load();
    const card = await waitFor(() => {
      const el = document.querySelector("[data-plan-card=pro]");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(card.textContent).toContain("Pro plan");
    expect(card.querySelector("[data-plan-primary]")?.textContent).toBe("Manage plan");
    expect(card.querySelector("[data-plan-secondary]")?.textContent).toBe("See Teams");
  });

  it("the account name carries the plan; on Free it opens Pro's checkout", async () => {
    const opened: string[] = [];
    const tracked: unknown[] = [];
    const { router } = await mountApp("/spaces", host(idle(), opened, tracked));
    await router.load();
    const plan = await waitFor(() => {
      const el = document.querySelector("[data-account-plan]");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(plan.textContent).toBe("Free plan · Upgrade");
    fireEvent.click(plan);
    await waitFor(() => expect(opened).toEqual(["https://app.cua.ai/buy/pro?from=app_account_menu"]));
    await waitFor(() => expect(features(tracked)).toContain("upsell_account_menu_click"));
  });

  it("a second machine over the relay: one line with Get Pro, shown once, closable", async () => {
    const opened: string[] = [];
    const tracked: unknown[] = [];
    const { router } = await mountApp("/machines", host(idle(), opened, tracked));
    await router.load();
    const prompt = await waitFor(() => {
      const el = document.querySelector("[data-upsell=machine]");
      expect(el).toBeTruthy();
      return el as HTMLElement;
    });
    expect(prompt.textContent).toContain("Two computers now. Free includes 2 live sessions and 20 remote hours a month. Pro makes it 5 and 200, for $18 a month.");
    await waitFor(() => expect(features(tracked)).toContain("upsell_machine_shown"));
    fireEvent.click(document.querySelector("[data-upsell=machine] [data-upsell-cta]")!);
    await waitFor(() => expect(opened).toEqual(["https://app.cua.ai/buy/pro?from=app_machine"]));
    fireEvent.click(document.querySelector("[data-upsell=machine] [data-upsell-dismiss]")!);
    expect(document.querySelector("[data-upsell=machine]")).toBeNull();
    await waitFor(() => expect(features(tracked)).toEqual(expect.arrayContaining(["upsell_machine_click", "upsell_machine_dismiss"])));
    cleanup();
    // Closed: it does not come back on the next visit.
    const again = await mountApp("/machines", host(idle()));
    await again.router.load();
    await act(() => new Promise((r) => setTimeout(r, 50)));
    expect(document.querySelector("[data-upsell=machine]")).toBeNull();
  });

  it("never during a live session", async () => {
    const { router } = await mountApp("/machines", host(idle({ liveSessions: 1 })));
    await router.load();
    await waitFor(() => expect(document.querySelector("[data-machine-row], [aria-label=Computers]")).toBeTruthy());
    await act(() => new Promise((r) => setTimeout(r, 50)));
    expect(document.querySelector("[data-upsell]")).toBeNull();
  });

  it("never during an agent run", async () => {
    const { router } = await mountApp("/machines", host(idle(), [], [], true));
    await router.load();
    await waitFor(() => expect(document.querySelector("[aria-label=Computers]")).toBeTruthy());
    await act(() => new Promise((r) => setTimeout(r, 80)));
    expect(document.querySelector("[data-upsell]")).toBeNull();
  });

  it("not on Pro: a second machine has nothing to sell", async () => {
    const { router } = await mountApp("/machines", host(idle({ plan: "pro" })));
    await router.load();
    await act(() => new Promise((r) => setTimeout(r, 50)));
    expect(document.querySelector("[data-upsell]")).toBeNull();
  });
});

describe.skipIf(!wasmBuilt)("<UpsellPrompt surface=share>", () => {
  let core: CoreClient;
  beforeAll(async () => {
    core = await testCore();
  });

  function view(usage: RelayUsage, active = true, opened: string[] = []) {
    const demo = createDemoAdapter({ latencyMs: 0 });
    demo.state.relayUsage = usage;
    const adapter: DataAdapter = {
      ...demo,
      call: (op, args) => {
        if (op === "session.openExternal") {
          opened.push((args as { url: string }).url);
          return Promise.resolve(null) as never;
        }
        return demo.call(op, args);
      },
    };
    return render(
      <BridgeProvider adapter={adapter} core={core}>
        <UpsellPrompt surface="share" active={active} />
      </BridgeProvider>,
    );
  }

  it("after sharing with a person: Teams, with its checkout", async () => {
    const opened: string[] = [];
    view(idle(), true, opened);
    const note = await screen.findByRole("note");
    expect(note.textContent).toContain("Sharing with people you work with? Teams adds team-owned computers, roles and device approval, from $30 a seat.");
    fireEvent.click(screen.getByRole("button", { name: "See Teams" }));
    await waitFor(() => expect(opened).toEqual(["https://app.cua.ai/buy/teams?from=app_share"]));
  });

  it("not before anything was shared, and not for a team already on Teams", async () => {
    view(idle(), false);
    await act(() => new Promise((r) => setTimeout(r, 30)));
    expect(document.querySelector("[data-upsell]")).toBeNull();
    cleanup();
    view(idle({ plan: "team" }));
    await act(() => new Promise((r) => setTimeout(r, 30)));
    expect(document.querySelector("[data-upsell]")).toBeNull();
  });
});
