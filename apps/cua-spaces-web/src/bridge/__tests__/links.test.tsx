// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createMemoryHistory, createRootRoute, createRouter, RouterProvider } from "@tanstack/react-router";
import { afterEach, describe, expect, it, vi } from "vitest";

import { createDemoAdapter } from "../adapters/demo";
import { NOT_ON_RELAY } from "../adapters/demo/links";
import { createWebkitAdapter } from "../adapters/webkit";
import { BridgeProvider, UnsupportedOperationError, linkEnds, needsRelaySetup, type DataAdapter } from "..";
import type { HostWindow } from "../detect";
import { SharedLinks } from "@/components/links/shared-links";
import { noCore } from "./testCore";

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

function inApp(adapter: DataAdapter, ui: React.ReactNode) {
  const router = createRouter({ routeTree: createRootRoute({ component: () => <>{ui}</> }), history: createMemoryHistory({ initialEntries: ["/"] }) });
  return render(
    <BridgeProvider adapter={adapter} core={noCore}>
      <RouterProvider router={router} />
    </BridgeProvider>,
  );
}

describe("links.*", () => {
  it("the demo host creates, lists and stops links in memory", async () => {
    const a = createDemoAdapter({ latencyMs: 0 });
    const l = await a.call("links.create", { target: "space:dev:3000" });
    expect(l).toMatchObject({ access: "private", kind: "space_port", target: "space:dev:3000", expiresAtUnix: null });
    const p = await a.call("links.create", { target: "./report.html", access: "public" });
    expect(p.expiresAtUnix).not.toBeNull();
    expect((await a.call("links.list", {})).links.map((x) => x.id)).toEqual([p.id, l.id]);
    expect(await a.call("links.stop", { id: l.id })).toEqual({ stopped: [l.id] });
    expect((await a.call("links.stop", { id: "all" })).stopped).toEqual([p.id]);
    expect((await a.call("links.list", {})).links).toEqual([]);
    await expect(a.call("links.stop", { id: "share-nope" })).rejects.toMatchObject({ code: "not_found" });
  });

  it("the SwiftUI and Electron hosts answer with the same methods", async () => {
    const postMessage = vi.fn(async (req: { id: string; method: string }) => ({ id: req.id, ok: true, result: { links: [] } }));
    const win = { webkit: { messageHandlers: { cua: { postMessage } } }, addEventListener: () => {}, removeEventListener: () => {} } as unknown as HostWindow;
    const a = createWebkitAdapter(win);
    expect(await a.call("links.list", {})).toEqual({ links: [] });
    expect(postMessage).toHaveBeenCalledWith(expect.objectContaining({ method: "links.list" }));
  });

  it("words", () => {
    expect(linkEnds({ expiresAtUnix: null } as never)).toBe("Until stopped");
    expect(linkEnds({ expiresAtUnix: 1_000 + 7200 } as never, 1_000_000)).toBe("2 h");
    expect(needsRelaySetup(NOT_ON_RELAY)).toBe(true);
    expect(needsRelaySetup("port 3000 is closed")).toBe(false);
  });
});

describe("Links without host support", () => {
  it("shows nothing where the host has no links", async () => {
    const demo = createDemoAdapter({ latencyMs: 0 });
    const adapter: DataAdapter = { ...demo, call: (op, args) => (String(op).startsWith("links.") ? Promise.reject(new UnsupportedOperationError("tauri", op)) : demo.call(op, args)) };
    const { container } = inApp(adapter, <SharedLinks />);
    await waitFor(() => expect(container.querySelector("section")).toBeNull());
  });
});

describe("Shared links", () => {
  it("lists, copies and stops", async () => {
    const writeText = vi.fn(async () => {});
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const demo = createDemoAdapter({ latencyMs: 0 });
    inApp(demo, <SharedLinks />);
    await screen.findByText("Nothing is shared.");
    const a = await demo.call("links.create", { target: "space:dev:3000", name: "Preview" });
    await demo.call("links.create", { target: "./report.html", access: "public" });
    // The list reads again on its own; trigger it with a remount.
    cleanup();
    inApp(demo, <SharedLinks />);
    await screen.findByText("Preview");
    expect(screen.getByRole("button", { name: "Stop all" })).toBeTruthy();
    fireEvent.click(document.querySelector(`[data-link="${a.id}"] [data-link-copy]`)!);
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(a.url));
    fireEvent.click(document.querySelector(`[data-link="${a.id}"] [data-link-stop]`)!);
    await waitFor(() => expect(screen.queryByText("Preview")).toBeNull());
    fireEvent.click(await screen.findByRole("button", { name: "Stop" }));
    await screen.findByText("Nothing is shared.");
  });
});
