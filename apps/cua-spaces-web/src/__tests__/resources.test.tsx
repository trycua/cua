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
  it("Home: no computer strips and no per-Space meters; nothing while there is room", async () => {
    await mountApp("/spaces", { failsCreates: false, listsGhost: false });
    await waitFor(() => expect(document.querySelector('[data-space-id="local:agent-sandbox"]')).not.toBeNull());
    // Give the resources a moment to arrive: still nothing to say.
    await act(() => new Promise((r) => setTimeout(r, 100)));
    expect(document.querySelector("[data-computer-resources]")).toBeNull();
    expect(document.querySelector("[data-computer-warnings]")).toBeNull();
    expect(document.querySelector("[data-space-meters]")).toBeNull();
    // A computer that is just offline is not a warning.
    expect(document.body.textContent).not.toMatch(/is offline/);
  });

  it("low disk: one quiet line for This Mac, and See what's using space lists This Mac's Spaces biggest first", async () => {
    const { router } = await mountApp("/spaces", { failsCreates: false, listsGhost: false, answer: lowDisk });
    const warning = await waitFor(() => {
      const el = document.querySelector<HTMLElement>('[data-computer-warnings] [data-low-disk="low"]');
      if (!el) throw new Error("no warning");
      return el;
    });
    expect(warning.textContent).toMatch(/^Disk almost full on This Mac \(41 GB left\)/);
    expect(document.querySelectorAll("[data-computer-warning]")).toHaveLength(1);
    fireEvent.click(warning.querySelector("[data-see-space-use]")!);
    // The list, This Mac only, sorted by size.
    await waitFor(() => expect(router.state.location.search).toEqual({ view: "list", on: "this-mac", sort: "size" }));
    await waitFor(() => expect(document.querySelector("[data-space-list]")).not.toBeNull());
    expect(document.querySelector('[data-sort-header="size"]')?.className).toContain("font-medium");
    const ids = Array.from(document.querySelectorAll("[data-space-row]")).map((t) => t.getAttribute("data-space-row"));
    expect(ids.length).toBeGreaterThan(0);
    expect(ids.every((id) => id?.startsWith("local:"))).toBe(true);
    expect(ids.slice(0, 2)).toEqual(["local:design-review", "local:agent-sandbox"]);
  });

  it("a Space's page: given vs in use, how it fits on this Mac, and the warning", async () => {
    await mountApp(`/spaces/${encodeURIComponent("local:design-review")}`, { failsCreates: false, listsGhost: false, answer: lowDisk, noThread: true });
    const panel = await waitFor(() => {
      const el = document.querySelector<HTMLElement>("[data-space-resources]");
      if (!el) throw new Error("no panel");
      return el;
    });
    const card = (label: string) => panel.querySelector(`[data-resource-card="${label}"]`)?.textContent ?? "";
    expect(card("Processor")).toContain("Given 4 of this Mac’s 12 cores");
    expect(card("Memory")).toContain("Given 8 GB of this Mac’s 64 GB");
    // Its own data on this Mac, of the size it was given.
    expect(card("Space for files")).toBe("Space for files19 of 80 GB");
    // Title, number, the one "Given" line and the bar: no explanations, no footer.
    expect(panel.textContent).not.toMatch(/Busy means|free for its apps|Uses about|Updates every|file names/);
    const fit = panel.querySelector("[data-disk-fit]")!;
    expect(fit.textContent).toContain("How it fits on this Mac");
    expect(fit.querySelector('[data-disk-part="yours"]')?.textContent).toBe("Your own files and apps · 929 GB");
    expect(fit.querySelector('[data-disk-part="space"]')?.textContent).toMatch(/ · 19 GB$/);
    expect(fit.querySelector('[data-disk-part="others"]')?.textContent).toBe("Other Spaces · 6 GB");
    // Each shared base once, with who shares it.
    const bases = Array.from(fit.querySelectorAll('[data-disk-part="base"]')).map((b) => b.textContent);
    expect(bases).toHaveLength(2);
    expect(bases[0]).toMatch(/ image · 25 GB, only this Space uses it$/);
    expect(bases[1]).toBe("linux 24.04 image · 4.1 GB, used by 1 Space");
    expect(fit.querySelector('[data-disk-part="free"]')?.textContent).toBe("Free · 41 GB");
    expect(fit.querySelector("[data-low-disk]")).not.toBeNull();
  });

  it("a container's page: its own data with no fake total", async () => {
    await mountApp(`/spaces/${encodeURIComponent("local:agent-sandbox")}`, { failsCreates: false, listsGhost: false, answer: lowDisk, noThread: true });
    const panel = await waitFor(() => {
      const el = document.querySelector<HTMLElement>("[data-space-resources]");
      if (!el) throw new Error("no panel");
      return el;
    });
    const files = panel.querySelector('[data-resource-card="Space for files"]')!;
    expect(files.textContent).toBe("Space for files6 GB");
    expect(files.querySelector("[data-bar-fill]")).toBeNull();
    expect(panel.querySelector('[data-resource-card="Memory"]')?.textContent).toContain("Given 4 GB of this Mac’s 64 GB");
  });

  it("another machine's running Space has no panel with only its size there", async () => {
    await mountApp(`/spaces/${encodeURIComponent("relay:mac-mini/qa-windows")}`, { failsCreates: false, listsGhost: false, noThread: true });
    await waitFor(() => expect(document.querySelector("[data-detail-title]")).not.toBeNull());
    await act(() => new Promise((r) => setTimeout(r, 50)));
    expect(document.querySelector("[data-space-resources]")).toBeNull();
  });
});
