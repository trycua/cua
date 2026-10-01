// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { Space } from "../model/types";
import type { SpacesListBridge } from "../native/spacesList";
import { Switcher } from "./Switcher";

const spaces: Space[] = [
  { id: "this-mac", name: "This machine", os: "macos", status: "local", detail: "Local", lastUsedAt: 2, scene: "blank" },
  { id: "cloud:aurora", name: "Aurora", os: "linux", status: "running", detail: "Agent testing", lastUsedAt: 1, scene: "blank" },
];

function fakeSpacesList(): SpacesListBridge {
  return {
    isNative: true,
    open: vi.fn(async () => {}),
    config: async () => ({ spaces: [] }),
    close: async () => {},
    openNewSpace: vi.fn(async () => {}),
    openSettings: vi.fn(async () => {}),
  };
}

function renderSwitcher() {
  const spacesList = fakeSpacesList();
  const onSelect = vi.fn();
  const onNew = vi.fn();
  render(
    <Switcher
      spaces={spaces}
      selectedId="cloud:aurora"
      focusIndex={0}
      notice={null}
      onFocus={() => {}}
      onSelect={onSelect}
      onNew={onNew}
      onCollapse={() => {}}
      spacesList={spacesList}
    />,
  );
  return { spacesList, onSelect, onNew };
}

// The notch panel holds only the Space tiles; lists, settings and creation are
// windows the shell opens.
describe("Switcher (the notch panel)", () => {
  it("a Space tile switches to that Space; nothing drills in inside the notch", () => {
    const { onSelect } = renderSwitcher();
    fireEvent.click(screen.getByRole("option", { name: /Aurora/ }));
    expect(onSelect).toHaveBeenCalledWith("cloud:aurora");
    expect(screen.queryByRole("button", { name: "Back to Spaces" })).toBeNull();
    expect(screen.getByRole("searchbox")).toBeInTheDocument();
  });

  it("controls have tooltips, and the whole search row focuses the field", () => {
    renderSwitcher();
    const tile = screen.getByRole("option", { name: /Aurora/ });
    expect(tile).toHaveAttribute("title", tile.getAttribute("aria-label"));
    expect(screen.getByRole("button", { name: "List view" })).toHaveAttribute("title");
    expect(screen.getByRole("button", { name: "Settings" })).toHaveAttribute("title", "Settings");
    // The search row is a <label>: a click anywhere in it (the glyph, the
    // padding) lands on the field.
    const row = screen.getByRole("searchbox").closest("label");
    expect(row).toHaveClass("switcher-search");
    expect(row).toHaveAttribute("title", "Search Spaces");
  });

  it("This machine opens its page in the main window", () => {
    const { spacesList, onSelect } = renderSwitcher();
    fireEvent.click(screen.getByRole("option", { name: /This machine/ }));
    expect(onSelect).not.toHaveBeenCalled();
    expect(spacesList.open).toHaveBeenCalledWith(expect.objectContaining({ selectedId: "this-mac" }));
  });

  it("the list button opens the main window on the current Space", () => {
    const { spacesList } = renderSwitcher();
    fireEvent.click(screen.getByRole("button", { name: "List view" }));
    expect(spacesList.open).toHaveBeenCalledWith(expect.objectContaining({ selectedId: "cloud:aurora" }));
  });

  it("the gear opens Settings in the main window, not a panel in the notch", () => {
    const { spacesList } = renderSwitcher();
    fireEvent.click(screen.getByRole("button", { name: "Settings" }));
    expect(spacesList.openSettings).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("dialog", { name: "Settings" })).toBeNull();
  });

  it("New asks for the New Space window", () => {
    const { onNew } = renderSwitcher();
    fireEvent.click(screen.getByRole("option", { name: /New/ }));
    expect(onNew).toHaveBeenCalledTimes(1);
  });
});
