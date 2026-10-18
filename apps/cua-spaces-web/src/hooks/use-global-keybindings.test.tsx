// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render } from "@testing-library/react";
import { createMemoryHistory, createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { WEBKIT_EVENT, WEBKIT_OPEN_ROUTE_EVENT, WEBKIT_OPEN_SETTINGS_EVENT } from "@/bridge/webkit-protocol";
import { isMacPlatform } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

import { appPath, useGlobalKeybindings } from "./use-global-keybindings";

afterEach(cleanup);
// The router scrolls on navigation; jsdom has no scrolling.
beforeEach(() => {
  window.scrollTo = (() => {}) as typeof window.scrollTo;
});

function Shell() {
  useGlobalKeybindings();
  return <Outlet />;
}

async function mountAt(path: string) {
  const root = createRootRoute({ component: Shell });
  const tree = root.addChildren(["/", "/spaces", "/spaces/$spaceId", "/settings"].map((p) => createRoute({ getParentRoute: () => root, path: p, component: () => null })));
  const router = createRouter({ routeTree: tree, history: createMemoryHistory({ initialEntries: [path] }) });
  await act(async () => {
    render(<RouterProvider router={router} />);
    await router.load();
  });
  return router;
}

const hostEvent = (event: string, payload?: unknown) => window.dispatchEvent(new CustomEvent(WEBKIT_EVENT, { detail: { event, payload } }));

describe("global keybindings", () => {
  it("opens Settings on ⌘, in the page", async () => {
    const router = await mountAt("/spaces");
    await act(async () => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: ",", metaKey: isMacPlatform(), ctrlKey: !isMacPlatform(), cancelable: true }));
    });
    expect(router.state.location.pathname).toBe("/settings");
  });

  it("opens Settings when the SwiftUI app's Settings command asks, with the page unfocused", async () => {
    const router = await mountAt("/spaces");
    useUiStore.getState().setPaletteOpen(true);
    await act(async () => {
      hostEvent(WEBKIT_OPEN_SETTINGS_EVENT);
    });
    expect(router.state.location.pathname).toBe("/settings");
    expect(useUiStore.getState().paletteOpen).toBe(false);
  });

  it("goes where the SwiftUI app's notch or menu bar item asks (New UI on)", async () => {
    const router = await mountAt("/settings");
    useUiStore.getState().setPaletteOpen(true);
    const dropped = encodeURIComponent(JSON.stringify(["Slack.app", "a b.txt"]));
    await act(async () => {
      hostEvent(WEBKIT_OPEN_ROUTE_EVENT, { path: `/spaces/s%201?dropped=${dropped}` });
    });
    expect(router.state.matches.at(-1)?.params).toEqual({ spaceId: "s 1" });
    expect(router.state.location.search).toEqual({ dropped: ["Slack.app", "a b.txt"] });
    expect(useUiStore.getState().paletteOpen).toBe(false);
    await act(async () => {
      hostEvent(WEBKIT_OPEN_ROUTE_EVENT, { path: "https://example.com/" });
      hostEvent(WEBKIT_OPEN_ROUTE_EVENT, { path: "//example.com/" });
      hostEvent(WEBKIT_OPEN_ROUTE_EVENT, null);
    });
    expect(router.state.matches.at(-1)?.params).toEqual({ spaceId: "s 1" });
  });

  it("takes only this app's paths from the host", () => {
    expect(appPath({ path: "/settings" })).toBe("/settings");
    expect(appPath({ path: "/keyvault?view=access" })).toBe("/keyvault?view=access");
    for (const path of ["settings", "//evil.example", "/\\evil.example", "https://evil.example/", 7]) expect(appPath({ path })).toBeNull();
    expect(appPath(null)).toBeNull();
  });

  it("ignores the host's other events and stops listening when it unmounts", async () => {
    const router = await mountAt("/spaces");
    await act(async () => {
      hostEvent("spaces.changed");
      hostEvent("spaces.newRequested");
    });
    expect(router.state.location.pathname).toBe("/spaces");
    cleanup();
    await act(async () => {
      hostEvent(WEBKIT_OPEN_SETTINGS_EVENT);
    });
    expect(router.state.location.pathname).toBe("/spaces");
  });
});
