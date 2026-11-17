// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { BridgeProvider, type DataAdapter } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { noCore } from "@/bridge/__tests__/testCore";

import { NetworkSheet } from "./network-sheet";
import { openNetworkSheet } from "./use-network";

afterEach(cleanup);

const RESEARCH = "local:research-swarm";
const WORKER = "local:swarm-worker-1";

function open(id: string) {
  const adapter: DataAdapter = createDemoAdapter({ latencyMs: 0, network: new Set(["network"]) });
  const router = createRouter({
    routeTree: createRootRoute({ component: () => <NetworkSheet spaceName={(x) => x} /> }),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
  render(
    <BridgeProvider adapter={adapter} core={noCore}>
      <RouterProvider router={router} />
    </BridgeProvider>,
  );
  return { adapter, id, ready: () => waitFor(() => expect(document.querySelector("[data-network-sheet]")).not.toBeNull()) };
}

const q = (sel: string) => document.querySelector<HTMLElement>(sel);
const extra = (id: string) => q(`[data-network-extra="${id}"]`);
const box = (id: string) => extra(id)!.querySelector<HTMLElement>('[role="checkbox"]')!;
const checked = (id: string) => box(id).getAttribute("aria-checked") === "true";
const click = (el: HTMLElement) => act(() => el.click());

describe("the Network picker", () => {
  it("shows Internet only with nothing ticked, and two independent boxes", async () => {
    const t = open(RESEARCH);
    act(() => openNetworkSheet(t.id));
    await t.ready();
    expect(q('[data-network-preset="internet"]')?.getAttribute("data-selected")).toBe("true");
    expect(q('[data-network-preset="localhost"]')).toBeNull();
    expect(q('[data-network-preset="lan"]')).toBeNull();
    expect(q("[data-network-extras]")?.textContent).toContain("Also allow");
    expect(extra("localhost")?.textContent).toContain("Services on this computer");
    expect(extra("localhost")?.textContent).toContain("Lets the Space use services running on this computer, like a database or dev server.");
    expect(extra("lan")?.textContent).toContain("Other devices on my network");
    expect(extra("lan")?.textContent).toContain("Lets the Space reach printers, routers, NAS and other computers on your network.");
    expect(checked("localhost")).toBe(false);
    expect(checked("lan")).toBe(false);
    expect(q("[data-network-warning]")).toBeNull();
    expect(q("[data-network-sheet]")?.getAttribute("data-preset")).toBe("internet");
  });

  it("ticks one box: its warning shows, and only localhost asks for ports", async () => {
    const t = open(RESEARCH);
    act(() => openNetworkSheet(t.id));
    await t.ready();
    click(box("lan"));
    expect(checked("lan")).toBe(true);
    expect(checked("localhost")).toBe(false);
    expect(q("[data-network-sheet]")?.getAttribute("data-preset")).toBe("lan");
    expect(q("[data-network-warning]")?.textContent).toContain("reach every computer on your network");
    expect(q("[data-network-ports]")).toBeNull();
    click(box("lan"));
    click(box("localhost"));
    expect(q("[data-network-sheet]")?.getAttribute("data-preset")).toBe("localhost");
    expect(q("[data-network-ports]")).not.toBeNull();
  });

  it("ticks both, saves the combination, and unticking one leaves the other", async () => {
    const t = open(RESEARCH);
    act(() => openNetworkSheet(t.id));
    await t.ready();
    click(box("localhost"));
    click(box("lan"));
    expect(checked("localhost") && checked("lan")).toBe(true);
    expect(q("[data-network-sheet]")?.getAttribute("data-preset")).toBe("localhost+lan");
    expect(document.querySelectorAll("[data-network-warning]").length).toBe(2);
    click(box("localhost"));
    expect(q("[data-network-sheet]")?.getAttribute("data-preset")).toBe("lan");
    click(box("localhost"));
    click(q("[data-network-save]")!);
    expect(t.adapter.network?.get(t.id)?.network.preset).toBe("localhost+lan");
  });

  it("hides the extras for No network and for Custom", async () => {
    const t = open(RESEARCH);
    act(() => openNetworkSheet(t.id));
    await t.ready();
    click(box("lan"));
    click(q('[data-network-preset="none"] button')!);
    expect(q("[data-network-extras]")).toBeNull();
    expect(q("[data-network-sheet]")?.getAttribute("data-preset")).toBe("none");
    click(q('[data-network-preset="internet"] button')!);
    expect(q("[data-network-sheet]")?.getAttribute("data-preset")).toBe("internet");
    expect(checked("lan")).toBe(false);
    click(q('[data-network-preset="custom"] button')!);
    expect(q("[data-network-extras]")).toBeNull();
  });

  it("keeps the extras when Internet only is clicked while they are ticked", async () => {
    const t = open(RESEARCH);
    act(() => openNetworkSheet(t.id));
    await t.ready();
    click(box("lan"));
    click(q('[data-network-preset="internet"] button')!);
    expect(checked("lan")).toBe(true);
  });

  it("stops a child from ticking what its parent does not allow", async () => {
    // swarm-worker-1 is a child of research-swarm; the demo host gives it Internet only here.
    const t = open(WORKER);
    act(() => openNetworkSheet(t.id));
    await t.ready();
    for (const id of ["localhost", "lan"]) {
      expect(box(id).hasAttribute("data-disabled")).toBe(true);
      expect(extra(id)?.textContent).toContain("More than research-swarm allows");
    }
    expect(q('[data-network-preset="none"] button')?.hasAttribute("disabled")).toBe(false);
  });
});
