// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { App } from "./App";
import { FIXTURE_NOW } from "./model/fixtures";
import { createFallbackBridge, type NativeBridge } from "./native/bridge";
import type { WindowMode } from "./native/types";
import { SETTINGS_KEYS } from "./state/settings";

beforeEach(() => {
  window.localStorage.clear();
});

function spyBridge(): NativeBridge & { modes: WindowMode[] } {
  const inner = createFallbackBridge("notched");
  const modes: WindowMode[] = [];
  return {
    ...inner,
    modes,
    setWindowMode: async (mode) => {
      modes.push(mode);
      return inner.setWindowMode(mode);
    },
  };
}

function renderApp(bridge = spyBridge()) {
  let clock = FIXTURE_NOW;
  const now = () => (clock += 1000);
  const user = userEvent.setup();
  render(<App bridge={bridge} now={now} collapseDelayMs={30} />);
  return { bridge, user };
}

const tileNames = () =>
  within(screen.getByRole("listbox"))
    .getAllByRole("option")
    .filter((el) => !el.classList.contains("tile-new"))
    .map((el) => el.getAttribute("aria-label")?.split(",")[0]);

describe("ambient -> switcher", () => {
  it("starts ambient with dots and count, expands on click and drives the native mode", async () => {
    const { bridge, user } = renderApp();
    const ambient = screen.getByRole("button", { name: /4 Spaces, 2 active/ });
    expect(ambient).toBeInTheDocument();
    await waitFor(() => expect(bridge.modes).toContain("ambient"));

    await user.click(ambient);

    expect(screen.getByRole("searchbox")).toBeInTheDocument();
    // Most-recent first. The grid marks no tile as "current" — a click drills
    // into a Space's windows rather than selecting it.
    expect(tileNames()).toEqual(["This Mac", "Windows QA", "Linux Build", "Research"]);
    expect(screen.queryByRole("option", { selected: true })).toBeNull();
    await waitFor(() => expect(bridge.modes.at(-1)).toBe("switcher"));
  });

  it("collapses with Escape", async () => {
    const { user } = renderApp();
    await user.click(screen.getByRole("button", { name: /4 Spaces/ }));
    await user.keyboard("{Escape}");
    // Island (the default theme) springs the panel back into the notch before
    // collapsing to the ambient tab, so the tab reappears asynchronously.
    expect(await screen.findByRole("button", { name: /4 Spaces/ })).toBeInTheDocument();
  });
});

describe("menu-bar presentation", () => {
  it("shows the ambient 'N Spaces' tab in the default notch mode", () => {
    renderApp();
    expect(screen.getByRole("button", { name: /4 Spaces, 2 active/ })).toBeInTheDocument();
  });

  it("hides the ambient tab when menu-bar mode is stored (status item is the entry)", () => {
    window.localStorage.setItem(SETTINGS_KEYS.menuBar, "true");
    renderApp();
    expect(screen.queryByRole("button", { name: /4 Spaces, 2 active/ })).toBeNull();
  });
});

describe("keyboard selection and MRU", () => {
  it("moves focus with arrows, selects with Enter, reorders MRU, confirms, then collapses", async () => {
    const { user } = renderApp();
    await user.click(screen.getByRole("button", { name: /4 Spaces/ }));

    // The filter box is focused on open; arrow keys then drive tile navigation.
    expect(screen.getByRole("searchbox")).toHaveFocus();
    await user.keyboard("{ArrowRight}{ArrowRight}");
    expect(screen.getByRole("option", { name: /^Linux Build/ })).toHaveFocus();

    await user.keyboard("{Enter}");

    // Switching still makes it the portal's current Space — which is what
    // re-orders the grid and seeds the popped-out list — it just is not drawn
    // as a selection on the tile any more.
    expect(screen.queryByRole("option", { selected: true })).toBeNull();
    expect(tileNames()).toEqual(["Linux Build", "This Mac", "Windows QA", "Research"]);
    expect(screen.getAllByRole("status")[0]).toHaveTextContent("Switching to Linux Build");

    await waitFor(() => expect(screen.getByRole("button", { name: /4 Spaces/ })).toBeInTheDocument());
    // Re-open: MRU order persists.
    await user.click(screen.getByRole("button", { name: /4 Spaces/ }));
    // The grid mounts as the notch opens: wait for it, as for the collapse.
    await waitFor(() => expect(tileNames()[0]).toBe("Linux Build"));
  });

  it("wraps from the New tile back to the first Space and reaches New with End", async () => {
    const { user } = renderApp();
    await user.click(screen.getByRole("button", { name: /4 Spaces/ }));
    await user.keyboard("{End}");
    expect(screen.getByRole("option", { name: /New Space/ })).toHaveFocus();
    await user.keyboard("{ArrowRight}");
    expect(screen.getByRole("option", { name: /^This Mac/ })).toHaveFocus();
    await user.keyboard("{ArrowLeft}");
    expect(screen.getByRole("option", { name: /New Space/ })).toHaveFocus();
  });
});

describe("New Space opens a window, never a notch panel", () => {
  function renderWithList() {
    const spacesList = {
      isNative: true,
      open: vi.fn(async () => {}),
      config: async () => ({ spaces: [] }),
      close: async () => {},
      openNewSpace: vi.fn(async () => {}),
      openSettings: vi.fn(async () => {}),
    };
    const user = userEvent.setup();
    let clock = FIXTURE_NOW;
    render(<App bridge={spyBridge()} spacesList={spacesList} now={() => (clock += 1000)} collapseDelayMs={30} />);
    return { spacesList, user };
  }

  it("Cmd/Ctrl+N asks the shell for the New Space window and folds the notch away", async () => {
    const { spacesList, user } = renderWithList();
    await user.click(screen.getByRole("button", { name: /4 Spaces/ }));
    await user.keyboard("{Control>}n{/Control}");
    expect(spacesList.openNewSpace).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("heading", { name: "New Fleet" })).toBeNull();
    expect(screen.queryByRole("dialog", { name: "New Space" })).toBeNull();
    expect(await screen.findByRole("button", { name: /4 Spaces/ })).toBeInTheDocument();
  });

  it("the + New tile does the same", async () => {
    const { spacesList, user } = renderWithList();
    await user.click(screen.getByRole("button", { name: /4 Spaces/ }));
    await user.click(screen.getByRole("option", { name: /New Space/ }));
    expect(spacesList.openNewSpace).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("searchbox")).toBeNull();
  });
});
