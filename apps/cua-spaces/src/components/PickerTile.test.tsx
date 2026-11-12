// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { PickerTile } from "../model/teleportFlow";
import { PickerTileView, tileIconKey, tileThumbnailKey } from "./PickerTile";

const tile: PickerTile = {
  id: "11",
  title: "Docs",
  help: "Google Chrome · Docs",
  disabled: false,
  selected: false,
  icon: { kind: "host", path: "/Applications/Google Chrome.app" },
  thumbnail: { kind: "host-window", windowId: 11 },
};

function renderTile(props: Partial<{ tile: PickerTile; icon: string | null; thumbnail: string | null }> = {}) {
  const onSelect = vi.fn();
  const onActivate = vi.fn();
  render(
    <ul>
      <PickerTileView
        tile={props.tile ?? tile}
        icon={props.icon ?? null}
        thumbnail={props.thumbnail ?? null}
        onSelect={onSelect}
        onActivate={onActivate}
      />
    </ul>,
  );
  return { onSelect, onActivate };
}

describe("PickerTileView", () => {
  it("shows the preview, the app's icon and one line of title", () => {
    renderTile({ icon: "data:image/png;base64,ICON", thumbnail: "data:image/png;base64,SHOT" });
    const option = screen.getByRole("option", { name: "Docs" });
    expect(option.querySelector("img.hp-card-img")).toHaveAttribute("src", "data:image/png;base64,SHOT");
    expect(screen.getByTestId("hp-card-badge-icon")).toHaveAttribute("src", "data:image/png;base64,ICON");
    expect(option).toHaveAccessibleDescription("Google Chrome · Docs");
  });

  it("shows the icon large when there is no window to preview, and nothing when there is no icon", () => {
    renderTile({ icon: "data:image/png;base64,ICON" });
    expect(screen.getByTestId("hp-card-fallback-icon")).toHaveAttribute("src", "data:image/png;base64,ICON");
  });

  it("never shows a placeholder glyph without an icon", () => {
    renderTile();
    expect(screen.queryByTestId("hp-card-badge-icon")).toBeNull();
    expect(screen.queryByTestId("hp-card-fallback-icon")).toBeNull();
  });

  it("selects on click and opens on double click", () => {
    const { onSelect, onActivate } = renderTile();
    fireEvent.click(screen.getByRole("option"));
    fireEvent.doubleClick(screen.getByRole("option"));
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onActivate).toHaveBeenCalledTimes(1);
  });

  it("keys icons per app and previews per window", () => {
    expect(tileIconKey({ kind: "guest", appName: "Firefox", appId: "FIREFOX", pid: 3 })).toBe(
      tileIconKey({ kind: "guest", appName: "firefox", appId: "firefox", pid: 9 }),
    );
    expect(tileIconKey({ kind: "none" })).toBe("");
    expect(tileThumbnailKey({ kind: "guest-window", windowId: "w-1", epoch: 2 })).toBe("guest:w-1@2");
  });
});
