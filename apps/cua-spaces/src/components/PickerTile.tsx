// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { PickerTile } from "../model/teleportFlow";

/**
 * One tile of the teleport picker's grid (every tab, the same as the
 * SwiftUI picker's): the live preview (the icon, large, when there is no
 * window to preview), the app's icon and one line of name. What the app can
 * take is the tooltip; a tile that cannot be chosen is dimmed. The tile is
 * the core's (`teleport::grid`); `icon` and `thumbnail` are what the shell
 * loaded for it, `null` while loading or when there is none.
 */
export function PickerTileView({
  tile,
  icon,
  thumbnail,
  onSelect,
  onActivate,
}: {
  tile: PickerTile;
  icon: string | null;
  thumbnail: string | null;
  onSelect: () => void;
  onActivate: () => void;
}) {
  return (
    <li>
      <button
        type="button"
        role="option"
        className="hp-card ta-tile"
        aria-selected={tile.selected}
        data-selected={tile.selected}
        aria-disabled={tile.disabled}
        disabled={tile.disabled}
        title={tile.help}
        aria-description={tile.help}
        onClick={onSelect}
        onDoubleClick={onActivate}
      >
        <span className="hp-card-preview">
          {thumbnail ? (
            <img className="hp-card-img" src={thumbnail} alt="" />
          ) : icon ? (
            // No window to preview: the app's icon, large.
            <span className="hp-card-fallback hp-card-fallback-icon">
              <img className="hp-card-fallback-img" src={icon} alt="" data-testid="hp-card-fallback-icon" />
            </span>
          ) : (
            <span className="hp-card-fallback" aria-hidden="true" />
          )}
        </span>
        <span className="hp-card-meta">
          {icon ? <img className="hp-card-badge hp-card-badge-icon" src={icon} alt="" data-testid="hp-card-badge-icon" /> : null}
          <span className="hp-card-title">{tile.title}</span>
        </span>
      </button>
    </li>
  );
}

/** A tile icon's cache key: this machine's app by path, a Space's by name
 * and id (windows of one app share one icon). Empty for none. */
export function tileIconKey(icon: PickerTile["icon"]): string {
  switch (icon.kind) {
    case "host":
      return `host\u001f${icon.path}`;
    case "guest":
      return `guest\u001f${icon.appName.toLowerCase()}\u001f${icon.appId.toLowerCase()}`;
    default:
      return "";
  }
}

/** A tile preview's cache key. Empty for none. */
export function tileThumbnailKey(thumbnail: PickerTile["thumbnail"]): string {
  switch (thumbnail.kind) {
    case "host-window":
      return `host:${thumbnail.windowId}`;
    case "guest-window":
      return `guest:${thumbnail.windowId}@${thumbnail.epoch}`;
    default:
      return "";
  }
}
