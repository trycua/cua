// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import { filterSpaces } from "../model/notch";
import type { Space } from "../model/types";
import { createSpacesListBridge, type SpacesListBridge } from "../native/spacesList";
import { NEW_SPACE_DROP_ID, NEW_TILE_INDEX, type Notice } from "../state/portal";
import { HotspotGlyph } from "./HotspotGlyph";
import { ListGlyph } from "./NavGlyphs";
import { SearchGlyph } from "./SearchGlyph";
import { GearGlyph } from "./SettingsPanel";
import { SpaceTile } from "./SpaceTile";

interface SwitcherProps {
  spaces: readonly Space[];
  /** The portal's current Space (MRU head). The notch grid no longer marks it
   * — a tile click drills in rather than selecting — but the popped-out list
   * window opens on it. */
  selectedId: string;
  focusIndex: number;
  notice: Notice | null;
  onFocus: (index: number) => void;
  onSelect: (id: string) => void;
  onNew: () => void;
  onCollapse: () => void;
  /** Pin a cloud Space as picture-in-picture; omit to hide the affordance. */
  onPin?: (space: Space) => void;
  /** Start the "move a local app into this Space" consent flow. */
  onShareApp?: (space: Space) => void;
  /** Delete a Space (a Space added by address is only removed). */
  onDelete?: (space: Space) => void;
  /** Turn a Space off (`on` false) or back on: the power button next to Delete. */
  onPower?: (space: Space, on: boolean) => void;
  /** True while this Mac is sharing its network to a Space. */
  hotspotActive?: boolean;
  /** The Space id currently borrowing this Mac's network, if any. */
  hotspotSpaceId?: string | null;
  /** Stop sharing the network (footer button when a hotspot is active). */
  onStopHotspot?: () => void;
  /** Window-drag in progress: tiles become clean drop targets. */
  dropMode?: boolean;
  /** Display name of the app being dragged (drop-mode header hint). */
  dragAppName?: string | null;
  /** Captured preview (data URL) of the dragged window; the tiny ghost echo. */
  dragGhost?: string | null;
  /** Id of the Space tile currently under the dragged window. */
  dropTargetId?: string | null;
  footer?: ReactNode;
  /** Shell bridge that opens the main window (Spaces list, New Space,
   * Settings). Nothing but the tiles renders in the notch panel itself. */
  spacesList?: SpacesListBridge;
}

const IS_MAC = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
const MOD = IS_MAC ? "⌘" : "Ctrl";

export function Switcher({
  spaces,
  selectedId,
  focusIndex,
  notice,
  onFocus,
  onSelect,
  onNew,
  onPin,
  onShareApp,
  onDelete,
  onPower,
  hotspotActive = false,
  hotspotSpaceId,
  onStopHotspot,
  dropMode = false,
  dragAppName,
  dragGhost,
  dropTargetId,
  footer,
  spacesList,
}: SwitcherProps) {
  const newRef = useRef<HTMLButtonElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const defaultSpacesList = useMemo(() => createSpacesListBridge(), []);
  const spacesListBridge = spacesList ?? defaultSpacesList;
  // The list view pops OUT: a real OS window owned by the shell, so it survives
  // the notch panel losing focus instead of dismissing like a popover.
  const openList = (focusId: string = selectedId) => {
    void spacesListBridge
      .open({
        spaces: spaces.map((space) => ({
          id: space.id,
          name: space.name,
          detail: space.detail,
          status: space.status,
          os: space.os,
        })),
        selectedId: focusId,
      })
      .catch(() => {});
  };
  // Focus the filter box when the switcher opens (runs after the child tiles'
  // roving-focus effect, so it wins) — unless we opened straight into drop mode.
  useEffect(() => {
    if (!dropMode) searchRef.current?.focus({ preventScroll: true });
    // Open-time only.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  // Filter the visible Spaces by name, OS, status, location or detail: the
  // core's rule (`notch.filter`), the same the SwiftUI notch uses.
  const filtered = useMemo(() => filterSpaces(spaces, query), [spaces, query]);
  useEffect(() => {
    if (focusIndex === NEW_TILE_INDEX && newRef.current && document.activeElement !== newRef.current) {
      newRef.current.focus({ preventScroll: true });
      newRef.current.scrollIntoView?.({ block: "nearest", inline: "nearest" });
    }
  }, [focusIndex]);

  return (
    <section className="shelf switcher" aria-label="Cua Spaces switcher" data-drop-mode={dropMode ? "true" : undefined}>
      <header className="shelf-header">
        {dropMode ? (
          <p className="shelf-drop-hint" role="status">
            Drop on a Space to Teleport{dragAppName ? ` ${dragAppName}` : ""}
          </p>
        ) : (
          <label className="switcher-search" title="Search Spaces">
            <SearchGlyph className="glyph switcher-search-glyph" />
            <input
              ref={searchRef}
              type="search"
              className="switcher-search-input"
              placeholder="Search"
              value={query}
              aria-label="Filter Cua Spaces by name, OS, status, or location"
              spellCheck={false}
              autoComplete="off"
              // The filter box is focused on open, so let navigation + shortcut
              // keys reach the portal's global handler (arrow tile nav, ⌘N,
              // Enter to switch, Escape to collapse) while plain typing stays
              // local. Escape clears the query first when there is one.
              onKeyDown={(event) => {
                if (event.key === "Escape" && query) {
                  event.stopPropagation();
                  setQuery("");
                  return;
                }
                const isShortcut = event.metaKey || event.ctrlKey;
                const isNav =
                  event.key.startsWith("Arrow") ||
                  event.key === "Enter" ||
                  event.key === "Home" ||
                  event.key === "End" ||
                  event.key === "Escape";
                if (!isShortcut && !isNav) event.stopPropagation();
              }}
              onChange={(event) => setQuery(event.target.value)}
            />
            {query && (
              <span className="switcher-search-count" aria-live="polite">
                {filtered.length}
              </span>
            )}
          </label>
        )}
      </header>

      <div className="tile-row" role="listbox" aria-label="Spaces, most recent first">
        {query.trim() && filtered.length === 0 ? (
          // No matches: fold the "New" action into the empty message as a link.
          <p className="switcher-empty" role="status">
            No Spaces match “{query.trim()}”.{" "}
            <button type="button" className="link-button" data-owns-enter onClick={onNew}>
              Create a new Space
            </button>
          </p>
        ) : (
          <>
            <button
              ref={newRef}
              type="button"
              role="option"
              aria-selected={false}
              className="tile tile-new"
              data-owns-enter
              // Marks the "+" tile as a drag drop target: dropping an app here
              // (not on an existing Space) creates a Space + teleports into it.
              data-space-id={NEW_SPACE_DROP_ID}
              data-drop-target={dropMode && dropTargetId === NEW_SPACE_DROP_ID ? "true" : undefined}
              tabIndex={focusIndex === NEW_TILE_INDEX ? 0 : -1}
              onFocus={() => onFocus(NEW_TILE_INDEX)}
              onClick={onNew}
              aria-label={dropMode ? "Drop here for a new Space" : `New Space (${MOD} N)`}
            >
              <span className="tile-frame tile-new-frame">
                <span className="tile-plus" aria-hidden="true">
                  +
                </span>
              </span>
              <span className="tile-caption">
                <span className="tile-name">New</span>
              </span>
            </button>
            {/* While teleporting, drop "This Mac": you cannot teleport to the local machine.
                Spaces borrowing this Mac's network sort first. */}
            {(dropMode ? filtered.filter((s) => s.status !== "local") : filtered)
              .slice()
              .sort((a, b) => Number(b.id === hotspotSpaceId) - Number(a.id === hotspotSpaceId))
              .map((space) => (
              <SpaceTile
                key={space.id}
                space={space}
                hotspot={space.id === hotspotSpaceId}
                focused={query.trim() ? false : focusIndex === spaces.indexOf(space)}
                onFocus={() => onFocus(spaces.indexOf(space))}
                // A tile switches to its Space (opens its desktop); "This
                // machine" opens its page in the main window. While a
                // window-drag is in flight the tile is the teleport target.
                onSelect={() =>
                  !dropMode && space.status === "local" ? openList(space.id) : onSelect(space.id)
                }
                onPin={onPin ? () => onPin(space) : undefined}
                onShareApp={onShareApp && space.status !== "local" ? () => onShareApp(space) : undefined}
                onDelete={onDelete ? () => onDelete(space) : undefined}
                onPower={onPower ? (on) => onPower(space, on) : undefined}
                dropMode={dropMode}
                dropTarget={dropMode && dropTargetId === space.id}
                dropGhost={dropMode && dropTargetId === space.id ? dragGhost ?? null : null}
              />
            ))}
          </>
        )}
      </div>

      {!dropMode && (
        <footer className="shelf-footer switcher-footer">
          {(
            <div className="switcher-space-actions">
              {/* List first, gear second (the list sits left of Settings). */}
              <button
                type="button"
                className="settings-gear"
                data-owns-enter
                aria-label="List view"
                title="Open the Cua Spaces window"
                onClick={() => openList()}
              >
                <ListGlyph className="glyph" />
              </button>
              <button
                type="button"
                className="settings-gear"
                data-owns-enter
                aria-label="Settings"
                title="Settings"
                onClick={() => void spacesListBridge.openSettings().catch(() => {})}
              >
                <GearGlyph className="glyph" />
              </button>
            </div>
          )}
          <div className="switcher-footer-main">
            {hotspotActive && (
              <button
                type="button"
                className="switcher-hotspot-stop"
                data-owns-enter
                onClick={onStopHotspot}
                title="Stop sharing your network"
              >
                <HotspotGlyph className="switcher-hotspot-stop-glyph" />
                Stop sharing network
              </button>
            )}
            {notice && (
              <span className="notice" data-kind={notice.kind} role="status">
                <span className="notice-mark" aria-hidden="true" />
                {notice.text}
              </span>
            )}
            {footer}
          </div>
        </footer>
      )}


    </section>
  );
}

