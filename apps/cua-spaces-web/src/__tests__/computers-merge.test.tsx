// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Machines and Spaces are one list of your computers (CUA-3551), on the real
// routes and the wasm core: the old Machines and Cua Driver addresses land
// on it (a notification, a bookmark, a `cua://add` link from an older Cua
// Spaces), with "Add a computer" open or the same computer's page when the
// link named one; Cua Driver is one of This Mac's settings, with a line under
// Computers in the sidebar, only where this Cua Spaces carries it.

import { act, cleanup, fireEvent, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";

import { HostError, UnsupportedOperationError } from "@/bridge/adapter";
import { wasmBuilt } from "@/bridge/__tests__/testCore";
import { driverLinkTarget, machinesLinkTarget } from "@/lib/old-links";
import { mountApp, stubBrowserApis, type TestHost } from "./app-harness";

afterEach(cleanup);
stubBrowserApis();

const demo: TestHost = { failsCreates: false, listsGhost: false };
/** A host with no driver operations (Electron, Tauri, a Mac build without it). */
const noDriver: TestHost = {
  ...demo,
  intercept: (op) => (op.startsWith("driver.") ? Promise.reject(new UnsupportedOperationError("webkit", op)) : undefined),
};
const q = (sel: string) => document.querySelector<HTMLElement>(sel);
const path = (r: { state: { location: { pathname: string } } }) => r.state.location.pathname;

describe("where an old link lands", () => {
  it("/machines opens the list, with Add a computer for add or join, and a named computer on its own page", () => {
    expect(machinesLinkTarget({})).toEqual({ to: "/spaces", search: {} });
    expect(machinesLinkTarget({ add: "1" })).toEqual({ to: "/spaces", search: { add: true } });
    expect(machinesLinkTarget({ join: "K7QM4XPB2RHD", relay: "https://r.example" })).toEqual({
      to: "/spaces",
      search: { join: "K7QM4XPB2RHD", relay: "https://r.example" },
    });
    expect(machinesLinkTarget({ machine: "studio" })).toEqual({ to: "/spaces/computer/$computerId", params: { computerId: "studio" } });
    // Adding wins over a named computer: the sheet is what the link asked for.
    expect(machinesLinkTarget({ machine: "studio", add: 1 })).toEqual({ to: "/spaces", search: { add: true } });
  });

  it("/driver opens Cua Driver under This Mac, or under the computer it names", () => {
    expect(driverLinkTarget({})).toEqual({ to: "/spaces/computer/$computerId/driver", params: { computerId: "this-mac" } });
    expect(driverLinkTarget({ machine: "mac-mini" })).toEqual({ to: "/spaces/computer/$computerId/driver", params: { computerId: "mac-mini" } });
  });
});

describe.skipIf(!wasmBuilt)("one list of your computers (real routes, wasm core)", () => {
  it("an old /machines link lands on the list", { timeout: 30000 }, async () => {
    const { router } = await mountApp("/machines", demo);
    await waitFor(() => expect(path(router)).toBe("/spaces"));
    await waitFor(() => expect(q("[data-spaces-tree]")).not.toBeNull());
  });

  it("a cua://add link from an older app opens Add a computer on Enter a code, filled in", { timeout: 30000 }, async () => {
    const { router } = await mountApp("/machines?join=K7QM4XPB2RHD", demo);
    await waitFor(() => expect(path(router)).toBe("/spaces"));
    await waitFor(() => expect(q("[data-add-computer]")?.getAttribute("data-add-computer")).toBe("enter"));
  });

  it("an old link that named a computer opens that computer's page", { timeout: 30000 }, async () => {
    const { router } = await mountApp("/machines?machine=this-mac", demo);
    await waitFor(() => expect(path(router)).toBe("/spaces/computer/this-mac"));
    await waitFor(() => expect(q('[data-machine-detail="this-mac"]')).not.toBeNull());
  });

  it("the old Trusted machines tab and an old /driver link land under This Mac", { timeout: 30000 }, async () => {
    const devices = await mountApp("/settings/devices", demo);
    await waitFor(() => expect(path(devices.router)).toBe("/spaces/computer/this-mac"));
    cleanup();
    const driver = await mountApp("/driver", demo);
    await waitFor(() => expect(path(driver.router)).toBe("/spaces/computer/this-mac/driver"));
  });

  it("Cua Driver is a setting on This Mac's page, opens under it, and goes back to it", { timeout: 30000 }, async () => {
    const { router } = await mountApp("/spaces", demo);
    // This Mac's row opens its page.
    const name = await waitFor(() => {
      const el = q('[data-tree-kind="computer"] [data-tree-name]');
      if (!el) throw new Error("no This Mac row");
      return el;
    });
    expect(name.textContent).toBe("This Mac");
    fireEvent.click(name, { detail: 0 });
    await waitFor(() => expect(path(router)).toBe("/spaces/computer/this-mac"));
    const row = await waitFor(() => {
      const el = q("[data-computer-driver-link]");
      if (!el) throw new Error("no Cua Driver row");
      return el;
    });
    expect(row.textContent).toContain("Cua Driver");
    act(() => row.click());
    await waitFor(() => expect(path(router)).toBe("/spaces/computer/this-mac/driver"));
    const back = await waitFor(() => {
      const el = q("[data-back-link]");
      if (!el?.textContent?.includes("This Mac")) throw new Error("no back link to This Mac");
      return el;
    });
    act(() => back.click());
    await waitFor(() => expect(path(router)).toBe("/spaces/computer/this-mac"));
  });

  it("the sidebar's Cua Driver line opens the page; This Mac's Cua Driver row says when it needs a person", { timeout: 30000 }, async () => {
    const stopped: TestHost = { ...demo, answer: (op, out) => (op === "driver.status" ? { ...(out as object), state: "stopped" } : out) };
    const { router } = await mountApp("/spaces/computer/this-mac", stopped);
    const row = await waitFor(() => {
      const el = q("[data-computer-driver-link] [data-driver-attention]");
      if (!el) throw new Error("no word on the Cua Driver row");
      return el;
    });
    expect(row.textContent).toBe("Stopped");
    const line = await waitFor(() => {
      const el = q("[data-sidebar-driver]");
      if (!el) throw new Error("no Cua Driver line in the sidebar");
      return el;
    });
    expect(line.querySelector("[data-driver-attention]")?.textContent).toBe("Stopped");
    act(() => line.click());
    await waitFor(() => expect(path(router)).toBe("/spaces/computer/this-mac/driver"));
  });

  it("when the account wants proof for This Mac, its page offers a code from another computer and opens Enter a code", { timeout: 30000 }, async () => {
    const refused = new HostError("To add this Mac to your account, enter a code from a computer you already added.", "failed", {
      title: "Confirm it’s you",
      details: "permission denied [code: new_computer_needs_proof]",
      actionLabel: "Use a code from another computer",
      kind: "needsProof",
      refusalCode: "new_computer_needs_proof",
    });
    await mountApp("/spaces/computer/this-mac", { ...demo, intercept: (op) => (op === "host.action" ? Promise.reject(refused) : undefined) });
    const toggle = await waitFor(() => {
      const el = document.querySelector<HTMLElement>('[data-machine-detail="this-mac"] [role="switch"]:not([disabled]):not([data-disabled])');
      if (!el) throw new Error("no switch on This Mac's page");
      return el;
    });
    act(() => toggle.click());
    const button = await waitFor(() => {
      const el = q('[data-host-failure="needs-proof"] [data-host-use-code]');
      if (!el) throw new Error("no Use a code button");
      return el;
    });
    expect(button.textContent).toBe("Use a code from another computer");
    act(() => button.click());
    await waitFor(() => expect(q("[data-add-computer]")?.getAttribute("data-add-computer")).toBe("enter"));
  });

  it("another computer's page has no Cua Driver row", { timeout: 30000 }, async () => {
    await mountApp("/spaces/computer/linux-box", demo);
    await waitFor(() => expect(q('[data-machine-detail="linux-box"]')).not.toBeNull());
    expect(q("[data-computer-driver]")).toBeNull();
  });

  it("where this Cua Spaces carries no driver, This Mac's page has no Cua Driver row", { timeout: 30000 }, async () => {
    await mountApp("/spaces/computer/this-mac", noDriver);
    await waitFor(() => expect(q('[data-machine-detail="this-mac"]')).not.toBeNull());
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(q("[data-computer-driver]")).toBeNull();
    expect(q("[data-sidebar-driver]")).toBeNull();
  });
});
