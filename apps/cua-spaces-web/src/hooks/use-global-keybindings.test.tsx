// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { act, cleanup, render } from "@testing-library/react";
import { createMemoryHistory, createRootRoute, createRoute, createRouter, Outlet, RouterProvider } from "@tanstack/react-router";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { WEBKIT_EVENT, WEBKIT_OPEN_SETTINGS_EVENT } from "@/bridge/webkit-protocol";
import { isMacPlatform } from "@/lib/utils";
import { useUiStore } from "@/stores/ui";

import { useGlobalKeybindings } from "./use-global-keybindings";

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
  const tree = root.addChildren(["/", "/spaces", "/settings"].map((p) => createRoute({ getParentRoute: () => root, path: p, component: () => null })));
  const router = createRouter({ routeTree: tree, history: createMemoryHistory({ initialEntries: [path] }) });
  await act(async () => {
    render(<RouterProvider router={router} />);
    await router.load();
  });
  return router;
}

const hostEvent = (event: string) => window.dispatchEvent(new CustomEvent(WEBKIT_EVENT, { detail: { event } }));

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
