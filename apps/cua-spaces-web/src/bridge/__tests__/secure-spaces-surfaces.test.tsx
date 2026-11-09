// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render, waitFor } from "@testing-library/react";
import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createDemoAdapter } from "../adapters/demo";
import { BridgeProvider, type DataAdapter } from "..";
import { SharingSettings } from "@/components/links/sharing-settings";
import { NetworkSheet } from "@/components/network/network-sheet";
import { BlockedNotice, ExitDownNotice, NetworkRow, SecurityRows } from "@/components/network/space-security";
import { openNetworkSheet } from "@/components/network/use-network";
import { noCore } from "./testCore";

afterEach(cleanup);

function inApp(adapter: DataAdapter, ui: React.ReactNode) {
  const router = createRouter({ routeTree: createRootRoute({ component: () => <>{ui}</> }), history: createMemoryHistory({ initialEntries: ["/"] }) });
  return render(
    <BridgeProvider adapter={adapter} core={noCore}>
      <RouterProvider router={router} />
    </BridgeProvider>,
  );
}

/** `?demo=network&...`. */
function networkDemo(): DataAdapter {
  return createDemoAdapter({ latencyMs: 0, network: new Set(["network", "net-blocked", "net-volume", "net-clipboard", "net-exit-down"]) });
}

const WORKER = "local:swarm-worker-1";

describe("Secure Spaces (always on)", () => {
  it("shows the Network row and the Access rows", async () => {
    inApp(
      networkDemo(),
      <>
        <NetworkRow spaceId={WORKER} />
        <SecurityRows spaceId={WORKER} />
      </>,
    );
    await waitFor(() => expect(document.querySelector("[data-security-rows]")).not.toBeNull());
    expect(document.querySelectorAll("[data-network-row]").length).toBe(2);
    expect(document.querySelector("[data-network-row]")?.textContent).toContain("Internet and this computer's services");
  });

  it("opens the Network sheet from the row", async () => {
    inApp(
      networkDemo(),
      <>
        <NetworkRow spaceId={WORKER} />
        <NetworkSheet spaceName={(id) => id} />
      </>,
    );
    const row = await waitFor(() => {
      const r = document.querySelector<HTMLElement>("[data-network-row]");
      expect(r).not.toBeNull();
      return r!;
    });
    act(() => row.click());
    await waitFor(() => expect(document.querySelector("[data-network-sheet]")).not.toBeNull());
    act(() => openNetworkSheet(""));
  });

  it("shows the blocked connection's notice and the exit-down line", async () => {
    inApp(
      networkDemo(),
      <>
        <BlockedNotice spaceId={WORKER} />
        <ExitDownNotice spaceId="local:research-swarm" />
      </>,
    );
    await waitFor(() => expect(document.querySelector("[data-network-blocked]")).not.toBeNull());
  });
});

describe("Public TCP shares (Labs) off", () => {
  it("shows no Sharing settings", async () => {
    const demo = createDemoAdapter({ latencyMs: 0 });
    const { container } = inApp(demo, <SharingSettings />);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 30));
    });
    expect(container.innerHTML).toBe("");
  });

  it("does not read the share settings from the host", async () => {
    const demo = createDemoAdapter({ latencyMs: 0 });
    const call = vi.fn(demo.call.bind(demo));
    inApp({ ...demo, call } as DataAdapter, <SharingSettings />);
    await act(async () => {
      await new Promise((r) => setTimeout(r, 30));
    });
    expect(call.mock.calls.filter(([op]) => String(op).startsWith("shares."))).toEqual([]);
  });
});

describe("Public TCP shares (Labs) on", () => {
  it("shows the Sharing settings with the cap and where shares go", async () => {
    inApp(createDemoAdapter({ latencyMs: 0, shares: "settings" }), <SharingSettings />);
    await waitFor(() => expect(document.querySelector("[data-sharing-settings]")).not.toBeNull());
    expect(document.querySelector("[data-sharing-used]")?.textContent).toBe("380 MB of 2 GB used today");
    expect(document.querySelector("[data-sharing-edge]")?.textContent).toMatch(/in AWS, about \$7 a month/);
  });
});
