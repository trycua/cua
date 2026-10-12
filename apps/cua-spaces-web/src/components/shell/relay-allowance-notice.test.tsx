// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { createDemoAdapter } from "@/bridge/adapters/demo";
import { demoRelayUsage } from "@/bridge/adapters/demo/relay-usage";
import { noCore } from "@/bridge/__tests__/testCore";
import { BridgeProvider, type DataAdapter, type RelayUsage } from "@/bridge";

import { RelayAllowanceNotice } from "./relay-allowance-notice";

afterEach(() => {
  cleanup();
  localStorage.clear();
});

const reached: RelayUsage = { ...demoRelayUsage(), hoursUsed: 20.2, softLimitReached: true, reasons: ["hours"] };

/** The demo host answering `usage`, recording the links it opens. */
function host(usage: RelayUsage | null) {
  const demo = createDemoAdapter({ latencyMs: 0 });
  demo.state.relayUsage = usage;
  const opened: string[] = [];
  const adapter: DataAdapter = {
    ...demo,
    call: (op, args) => {
      if (op === "session.openExternal") opened.push((args as { url: string }).url);
      return demo.call(op, args);
    },
  };
  const view = () =>
    render(
      <BridgeProvider adapter={adapter} core={noCore}>
        <RelayAllowanceNotice />
      </BridgeProvider>,
    );
  return { demo, opened, view };
}

describe("<RelayAllowanceNotice>", () => {
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
    expect(notice.textContent).toContain("You've used your free relay allowance for this month. Your Spaces keep working.");
    fireEvent.click(screen.getByRole("button", { name: "Upgrade" }));
    await waitFor(() => expect(opened).toEqual(["https://cua.ai/pricing"]));
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
