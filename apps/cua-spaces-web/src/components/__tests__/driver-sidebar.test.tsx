// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Cua Driver is easy to find again: a line under Computers in the sidebar and
// a palette entry, both only where this Cua Spaces carries the driver, and a
// quiet word beside the line only when the driver needs a person.

import { createMemoryHistory, createRootRoute, createRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { act, cleanup, fireEvent, render, waitFor } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { BridgeProvider, type DriverStatus } from "@/bridge";
import { UnsupportedOperationError, type DataAdapter } from "@/bridge/adapter";
import { createDemoAdapter } from "@/bridge/adapters/demo";
import { unavailableCore } from "@/bridge/core";
import { driverAttention } from "@/components/driver/copy";
import { CommandPalette } from "@/components/shell/command-palette";
import { SidebarDriverLine } from "@/components/shell/sidebar";
import { ToastProvider } from "@/components/ui/toast";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useUiStore } from "@/stores/ui";

afterEach(() => {
  cleanup();
  useUiStore.getState().setPaletteOpen(false);
});
beforeEach(() => {
  window.matchMedia ??= ((query: string) => ({
    matches: false,
    media: query,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
    onchange: null,
  })) as unknown as typeof window.matchMedia;
});

const DRIVER_PATH = "/spaces/computer/this-mac/driver";

function mount(adapter: DataAdapter, ui: ReactNode) {
  const root = createRootRoute({ component: () => <>{ui}</> });
  const driverPage = createRoute({ getParentRoute: () => root, path: "/spaces/computer/$computerId/driver", component: () => <div data-driver-page="" /> });
  const router = createRouter({ routeTree: root.addChildren([driverPage]), history: createMemoryHistory({ initialEntries: ["/"] }) });
  render(
    <BridgeProvider adapter={adapter} core={unavailableCore("test: no core")}>
      <ToastProvider>
        <TooltipProvider>
          <RouterProvider router={router} />
        </TooltipProvider>
      </ToastProvider>
    </BridgeProvider>,
  );
  return router;
}

const demo = () => createDemoAdapter({ latencyMs: 0, stepMs: 0 });
type DemoAdapter = ReturnType<typeof demo>;

/** The demo host, with `driver.status` rewritten (a state the demo can't reach by itself), or without the driver at all. */
function withStatus(base: DemoAdapter, change: ((s: DriverStatus) => DriverStatus) | "none"): DataAdapter {
  return Object.assign(Object.create(base) as DataAdapter, {
    call: (async (op: string, args: unknown) => {
      if (op.startsWith("driver.") && change === "none") throw new UnsupportedOperationError("webkit", op);
      const out = await (base.call as (o: string, a: unknown) => Promise<unknown>).call(base, op, args);
      return op === "driver.status" && change !== "none" ? change(out as DriverStatus) : out;
    }) as DataAdapter["call"],
  });
}

const line = () => document.querySelector<HTMLAnchorElement>("[data-sidebar-driver]");
const attention = () => document.querySelector("[data-driver-attention]");
const settle = () => act(() => new Promise((r) => setTimeout(r, 30)));

describe("the Cua Driver line under Computers", () => {
  it("shows where this Cua Spaces carries the driver, opens its page, and is quiet while it works", async () => {
    const router = mount(demo(), <SidebarDriverLine />);
    const el = await waitFor(() => {
      if (!line()) throw new Error("no Cua Driver line");
      return line()!;
    });
    expect(el.textContent).toBe("Cua Driver");
    expect(attention()).toBeNull();
    act(() => el.click());
    await waitFor(() => expect(router.state.location.pathname).toBe(DRIVER_PATH));
  });

  it("is not drawn where this Cua Spaces carries no driver", async () => {
    mount(withStatus(demo(), "none"), <SidebarDriverLine />);
    await settle();
    expect(line()).toBeNull();
  });

  it("says Needs access when a System Settings switch is off", async () => {
    const d = demo();
    d.state.host.permissions = [
      {
        id: "accessibility",
        label: "Accessibility",
        instructions: "",
        settingsUrl: "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        granted: false,
      },
    ];
    mount(d, <SidebarDriverLine />);
    await waitFor(() => expect(attention()?.getAttribute("data-driver-attention")).toBe("needs-permission"));
    expect(attention()?.textContent).toBe("Needs access");
  });

  it.each([
    ["stopped", "Stopped"],
    ["failed", "Not answering"],
  ] as const)("says %s as %s", async (state, word) => {
    mount(
      withStatus(demo(), (s) => ({ ...s, state })),
      <SidebarDriverLine />,
    );
    await waitFor(() => expect(attention()?.getAttribute("data-driver-attention")).toBe(state));
    expect(attention()?.textContent).toBe(word);
  });

  it("says Update ready when a newer Cua Driver is ready", async () => {
    const d = demo();
    d.state.driver.update = { version: "0.38.1", note: null };
    mount(d, <SidebarDriverLine />);
    await waitFor(() => expect(attention()?.textContent).toBe("Update ready"));
  });

  it("stays quiet while paused on purpose or starting", async () => {
    for (const state of ["paused", "starting"] as const) {
      mount(
        withStatus(demo(), (s) => ({ ...s, state })),
        <SidebarDriverLine />,
      );
      await waitFor(() => expect(line()).not.toBeNull());
      await settle();
      expect(attention()).toBeNull();
      cleanup();
    }
  });
});

describe("what needs a person, most urgent first", () => {
  const base: DriverStatus = {
    machineId: "this-mac",
    machineName: "This Mac",
    thisMachine: true,
    state: "ready",
    version: "0.38.0",
    update: null,
    lastTest: null,
    lastError: null,
    permissions: [],
    apps: null,
    activity: [],
    canPause: true,
  };
  const update = { version: "0.38.1", note: null };

  it("puts a broken driver ahead of an update, and says nothing while it works", () => {
    expect(driverAttention(base)).toBeNull();
    expect(driverAttention({ ...base, state: "needs-permission", update })?.kind).toBe("needs-permission");
    expect(driverAttention({ ...base, state: "stopped", update })?.kind).toBe("stopped");
    expect(driverAttention({ ...base, update })).toEqual({ kind: "update", word: "Update ready", tip: "Cua Driver 0.38.1 is ready. Open it to update." });
  });
});

describe("Cmd+K finds Cua Driver", () => {
  const search = async (query: string) => {
    const input = await waitFor(() => {
      const el = document.querySelector<HTMLInputElement>("[data-command-palette] input");
      if (!el) throw new Error("no palette");
      return el;
    });
    fireEvent.change(input, { target: { value: query } });
    return input;
  };
  const option = () => document.querySelector("#palette-nav\\:driver");

  it.each(["driver", "permissions", "accessibility", "screen recording", "computer use"])("by %s, and opens its page", async (query) => {
    const router = mount(demo(), <CommandPalette />);
    act(() => useUiStore.getState().setPaletteOpen(true));
    const input = await search(query);
    await waitFor(() => expect(option()?.textContent).toContain("Cua Driver"));
    fireEvent.keyDown(input, { key: "Enter" });
    // Enter runs the first match; the driver entry is the only one for these words.
    await waitFor(() => expect(router.state.location.pathname).toBe(DRIVER_PATH));
  });

  it("has no entry where this Cua Spaces carries no driver", async () => {
    mount(withStatus(demo(), "none"), <CommandPalette />);
    act(() => useUiStore.getState().setPaletteOpen(true));
    await search("driver");
    await settle();
    expect(option()).toBeNull();
  });
});
