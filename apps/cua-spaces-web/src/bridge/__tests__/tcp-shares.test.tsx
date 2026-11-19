// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { afterEach, describe, expect, it } from "vitest";

import { createDemoAdapter, demoOptionsFromSearch } from "../adapters/demo";
import { SHARE_CONSENT_TEXT } from "../adapters/demo/links";
import { BridgeProvider, dataWords, type DataAdapter } from "..";
import { SharedLinks } from "@/components/links/shared-links";
import { usedWords } from "@/components/links/sharing-settings";
import { allowProblem, parseAllow, whoWords } from "@/components/links/tcp-words";
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

const demo = (shares?: "consent" | "routes" | "settings" | "cap") => createDemoAdapter({ latencyMs: 0, shares });

describe("the demo host's public TCP shares", () => {
  it("shares live through Cua by default, with a route line", async () => {
    const a = demo();
    const s = await a.call("links.create", { target: "space:mc:25565", tcp: true, service: "minecraft" });
    expect(s).toMatchObject({ route: "relay", routeLine: "Through Cua", waiting: null, allow: [] });
    expect(s.tcpConnectWith).toMatch(/\.m\.cua\.sh$/);
  });

  it("keeps who may connect", async () => {
    const a = demo();
    const s = await a.call("links.create", { target: "space:mc:25565", tcp: true, allow: ["203.0.113.0/24"] });
    expect(s.allow).toEqual(["203.0.113.0/24"]);
  });

  it("asks before the first share through a share edge, and creates the edge on yes", async () => {
    const a = demo("consent");
    const w = await a.call("links.create", { target: "space:mc:25565", tcp: true, service: "minecraft" });
    expect(w).toMatchObject({ waiting: "consent", waitingText: SHARE_CONSENT_TEXT, tcpConnectWith: null });
    expect((await a.call("shares.settings.get", {})).edgeHost).toBeNull();
    const live = await a.call("links.approve", { id: w.id });
    expect(live).toMatchObject({ waiting: null, route: "edge", routeLine: "Through your AWS account" });
    expect(live.tcpConnectWith).toMatch(/amazonaws\.com:\d+$/);
    const s = await a.call("shares.settings.get", {});
    expect(s).toMatchObject({ edgeCloud: "AWS", edgeMonthlyUsd: 7, edgeConsent: ["AWS"] });
  });

  it("a share with consent given goes live at once", async () => {
    const a = demo("consent");
    const l = await a.call("links.create", { target: "space:mc:25565", tcp: true, consent: true });
    expect(l).toMatchObject({ waiting: null, route: "edge" });
  });

  it("turning down a waiting share removes it", async () => {
    const a = demo("consent");
    const w = await a.call("links.create", { target: "space:mc:25565", tcp: true });
    expect(await a.call("links.deny", { id: w.id })).toEqual({ denied: w.id });
    expect((await a.call("links.list", {})).links).toEqual([]);
    await expect(a.call("links.deny", { id: w.id })).rejects.toMatchObject({ code: "not_found" });
  });

  it("approving an agent's share can allow that Space from now on", async () => {
    const a = demo("routes");
    const waiting = (await a.call("links.list", {})).links.find((l) => l.waiting === "approval")!;
    expect(waiting.waitingText).toMatch(/wants to share port 25565/);
    const live = await a.call("links.approve", { id: waiting.id, always: true });
    expect(live.waiting).toBeNull();
    expect((await a.call("shares.settings.get", {})).alwaysAllowSpaces).toEqual(["local:mc"]);
  });

  it("saves the settings but keeps the read-only fields", async () => {
    const a = demo("cap");
    const before = await a.call("shares.settings.get", {});
    expect(before.usedToday).toBeGreaterThan(1.3 * 1024 ** 3);
    const saved = await a.call("shares.settings.set", { settings: { ...before, dailyCapMib: 5120, approveAgentShares: false, usedToday: 0, edgeHost: null } });
    expect(saved).toMatchObject({ dailyCapMib: 5120, approveAgentShares: false, usedToday: before.usedToday, edgeHost: before.edgeHost });
    expect(await a.call("shares.settings.get", {})).toEqual(saved);
  });

  it("removing the share edge ends the shares through it", async () => {
    const a = demo("routes");
    const edge = (await a.call("links.list", {})).links.filter((l) => l.route === "edge").map((l) => l.id);
    expect(edge).toHaveLength(1);
    expect(await a.call("shares.edge.remove", {})).toEqual({ ended: edge });
    const s = await a.call("shares.settings.get", {});
    expect(s).toMatchObject({ edgeHost: null, edgeCloud: null, edgeMonthlyUsd: null });
    expect((await a.call("links.list", {})).links.some((l) => l.route === "edge")).toBe(false);
  });

  it("turns Labs on for the shares scenes", async () => {
    expect(demoOptionsFromSearch("?demo=shares-routes").shares).toBe("routes");
    expect((await demo("routes").call("experiments.get", {})).tcpShares).toBe(true);
    expect((await demo().call("experiments.get", {})).tcpShares).toBeFalsy();
  });
});

describe("Shared links with public TCP shares", () => {
  it("shows each share's route, and an agent's ask as waiting for you", async () => {
    inApp(demo("routes"), <SharedLinks />);
    await screen.findByText("203.0.113.24:26500");
    expect(screen.getByText("ec2-198-51-100-7.us-west-2.compute.amazonaws.com:27003")).toBeTruthy();
    expect(screen.getByText("mc-1a2b3c4d.m.cua.sh")).toBeTruthy();
    expect(document.querySelectorAll('[data-link-route="host"], [data-link-route="edge"]')).toHaveLength(2);
    expect(document.querySelector('[data-link-route="relay"]')?.textContent).toBe("Through Cua");
    expect(screen.getByText("Waiting for you")).toBeTruthy();
    expect(screen.getByText(/wants to share port 25565 publicly/)).toBeTruthy();
  });

  it("Approve with Always allow goes live and remembers the Space", async () => {
    const a = demo("routes");
    inApp(a, <SharedLinks />);
    await screen.findByText("Waiting for you");
    fireEvent.click(document.querySelector("[data-waiting-always]")!);
    fireEvent.click(screen.getByRole("button", { name: "Approve" }));
    await waitFor(() => expect(screen.queryByText("Waiting for you")).toBeNull());
    expect((await a.call("shares.settings.get", {})).alwaysAllowSpaces).toEqual(["local:mc"]);
  });

  it("Turn down removes the waiting share", async () => {
    inApp(demo("routes"), <SharedLinks />);
    await screen.findByText("Waiting for you");
    fireEvent.click(screen.getByRole("button", { name: "Turn down" }));
    await waitFor(() => expect(screen.queryByText("Waiting for you")).toBeNull());
  });
});

describe("words", () => {
  it("says what was used today", () => {
    expect(dataWords(Math.round(1.4 * 1024 ** 3))).toBe("1.4 GB");
    expect(dataWords(300 * 1024 ** 2)).toBe("300 MB");
    expect(usedWords(Math.round(1.4 * 1024 ** 3), 2048)).toBe("1.4 GB of 2 GB used today");
    expect(usedWords(Math.round(1.4 * 1024 ** 3), 0)).toBe("1.4 GB used today");
  });

  it("reads typed addresses", () => {
    expect(parseAllow("203.0.113.0/24, 198.51.100.7\n10.0.0.0/8")).toEqual(["203.0.113.0/24", "198.51.100.7", "10.0.0.0/8"]);
    expect(allowProblem(["203.0.113.0/24", "198.51.100.7"])).toBeNull();
    expect(allowProblem(["999.1.1.1"])).toMatch(/not an address/);
    expect(allowProblem(["10.0.0.0/40"])).toMatch(/not an address/);
    expect(whoWords([])).toBe("Anyone");
    expect(whoWords(["10.0.0.0/8", "198.51.100.7"])).toBe("Only 2 addresses");
  });
});
