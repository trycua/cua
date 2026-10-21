// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Processor, memory and disk on Home and a Space's page (CUA-1880): the
// numbers' store, the strips, the low-disk warning and its "See what's
// using space" (real routes, wasm core, the demo host).

import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { BridgeProvider, HostError, RESOURCES_HIDDEN_MS, RESOURCES_VISIBLE_MS, useResources, type DataAdapter, type ResourcesSnapshot } from "@/bridge";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { DEMO_DISK_FREE, demoDiskWarning } from "@/bridge/adapters/demo/resources";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { mountApp, stubBrowserApis } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const lowDisk = (op: string, out: unknown) => {
  if (op !== "resources.get" || !out) return out;
  const snap = out as ResourcesSnapshot;
  const warning = demoDiskWarning(DEMO_DISK_FREE.low, snap.thisMac.diskTotal);
  return { ...snap, thisMac: { ...snap.thisMac, diskFree: DEMO_DISK_FREE.low, warning }, warning };
};

describe("the resources store", () => {
  function Probe({ seen }: { seen: { snap?: ResourcesSnapshot | null } }) {
    seen.snap = useResources();
    return null;
  }

  it("reads every 10 s while visible and every 5 minutes while hidden, saying which", async () => {
    vi.useFakeTimers();
    try {
      const demo = createDemoAdapter({ latencyMs: 0 });
      const calls: boolean[] = [];
      const adapter: DataAdapter = { ...demo, call: (op, args) => (op === "resources.get" && calls.push((args as { visible: boolean }).visible), demo.call(op, args)) };
      const seen: { snap?: ResourcesSnapshot | null } = {};
      render(
        <BridgeProvider adapter={adapter}>
          <Probe seen={seen} />
        </BridgeProvider>,
      );
      await act(() => vi.advanceTimersByTimeAsync(10));
      expect(calls).toEqual([true]);
      expect(seen.snap?.thisMac.id).toBe("this-mac");
      await act(() => vi.advanceTimersByTimeAsync(RESOURCES_VISIBLE_MS));
      expect(calls.length).toBe(2);
      // Hidden: the slow pace, and the read says so.
      Object.defineProperty(document, "visibilityState", { value: "hidden", configurable: true });
      document.dispatchEvent(new Event("visibilitychange"));
      await act(() => vi.advanceTimersByTimeAsync(RESOURCES_VISIBLE_MS * 3));
      expect(calls.length).toBe(2);
      await act(() => vi.advanceTimersByTimeAsync(RESOURCES_HIDDEN_MS));
      expect(calls.at(-1)).toBe(false);
      // Back: at once.
      Object.defineProperty(document, "visibilityState", { value: "visible", configurable: true });
      const before = calls.length;
      document.dispatchEvent(new Event("visibilitychange"));
      await act(() => vi.advanceTimersByTimeAsync(5));
      expect(calls.length).toBe(before + 1);
      expect(calls.at(-1)).toBe(true);
    } finally {
      Object.defineProperty(document, "visibilityState", { value: "visible", configurable: true });
      vi.useRealTimers();
    }
  });

  it("a host without the operation reads as null, and is not asked again", async () => {
    vi.useFakeTimers();
    try {
      const demo = createDemoAdapter({ latencyMs: 0 });
      let asked = 0;
      const adapter: DataAdapter = {
        ...demo,
        call: (op, args) => {
          if (op !== "resources.get") return demo.call(op, args);
          asked += 1;
          return Promise.reject(new HostError("resources.get", "unimplemented"));
        },
      };
      const seen: { snap?: ResourcesSnapshot | null } = {};
      render(
        <BridgeProvider adapter={adapter}>
          <Probe seen={seen} />
        </BridgeProvider>,
      );
      await act(() => vi.advanceTimersByTimeAsync(RESOURCES_VISIBLE_MS * 3));
      expect(seen.snap).toBeNull();
      expect(asked).toBe(1);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe.skipIf(!wasmBuilt)("resources on Home and a Space's page", () => {
  it("Home: This Mac's strip, the Mac mini's summary, and each Space's meters; no warning with room", async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    const mac = await waitFor(() => {
      const el = document.querySelector<HTMLElement>('[data-computer-resources="this-mac"]');
      if (!el) throw new Error("no This Mac strip");
      return el;
    });
    expect(mac.textContent).toContain("Processor");
    expect(mac.textContent).toContain("12 cores");
    expect(mac.textContent).toContain("Space for files");
    expect(mac.textContent).toContain("Your extra Spaces use 6 cores, 12 GB of memory and about 25 GB for files.");
    expect(document.querySelector('[data-computer-resources="mac-mini"]')?.textContent).toMatch(/Updated .* ago\. Your extra Spaces there use/);
    // The Linux box sent no summary.
    expect(document.querySelector('[data-computer-resources="linux-box"]')).toBeNull();
    expect(document.querySelector("[data-low-disk]")).toBeNull();
    const tile = document.querySelector('[data-space-id="local:agent-sandbox"]')!;
    await waitFor(() => expect(tile.querySelector("[data-space-meters]")?.textContent).toBe("StoppedOffAbout 6 GB"));
    // Other computers' Spaces have no per-Space numbers.
    expect(document.querySelector('[data-space-id="relay:mac-mini/qa-windows"] [data-space-meters]')).toBeNull();
  });

  it("low disk: the warning on This Mac, and See what's using space lists the Spaces biggest first", async () => {
    const { router } = await mountApp("/spaces", { failsCreates: false, listsGhost: false, answer: lowDisk });
    const warning = await waitFor(() => {
      const el = document.querySelector<HTMLElement>('[data-computer-resources="this-mac"] [data-low-disk="low"]');
      if (!el) throw new Error("no warning");
      return el;
    });
    expect(warning.textContent).toContain("This Mac is almost out of space. Delete old Spaces or snapshots.");
    fireEvent.click(warning.querySelector("[data-see-space-use]")!);
    await waitFor(() => expect(router.state.location.search).toEqual({ sort: "size" }));
    await waitFor(() => expect(document.querySelector("[data-sorted-by-size]")).not.toBeNull());
    const ids = Array.from(document.querySelectorAll("[data-space-id]")).map((t) => t.getAttribute("data-space-id"));
    expect(ids.slice(0, 2)).toEqual(["local:design-review", "local:agent-sandbox"]);
  });

  it("a Space's page: given vs in use, how it fits on this Mac, and the warning", async () => {
    await mountApp(`/spaces/${encodeURIComponent("local:design-review")}`, { failsCreates: false, listsGhost: false, answer: lowDisk });
    const panel = await waitFor(() => {
      const el = document.querySelector<HTMLElement>("[data-space-resources]");
      if (!el) throw new Error("no panel");
      return el;
    });
    const card = (label: string) => panel.querySelector(`[data-resource-card="${label}"]`)?.textContent ?? "";
    expect(card("Processor")).toContain("Given 4 of this Mac’s 12 cores");
    expect(card("Memory")).toContain("Given 8 GB of this Mac’s 64 GB");
    expect(card("Space for files")).toContain("34 of 80 GB");
    expect(card("Space for files")).toContain("Uses about 19 GB on this Mac (shares space with other copies).");
    const fit = panel.querySelector("[data-disk-fit]")!;
    expect(fit.textContent).toContain("How it fits on this Mac");
    expect(fit.querySelector('[data-disk-part="own"]')?.textContent).toBe("Your own files and apps · 934 GB");
    expect(fit.querySelector('[data-disk-part="free"]')?.textContent).toBe("Free · 41 GB");
    expect(fit.querySelector("[data-low-disk]")).not.toBeNull();
  });

  it("another computer's Space has no panel without its numbers", async () => {
    await mountApp(`/spaces/${encodeURIComponent("relay:mac-mini/qa-windows")}`, { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(document.querySelector("[data-detail-title]")).not.toBeNull());
    await act(() => new Promise((r) => setTimeout(r, 50)));
    expect(document.querySelector("[data-space-resources]")).toBeNull();
  });
});
