// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useMemo, useState } from "react";

import { type CatalogEntry, entryFromCore } from "@trycua/cua/teleport";

import type { OpenWindow, PickerTab, RemoteWindow, TeleportPickerConfig } from "../model/teleport";
import { createTeleportBridge, type TeleportBridge } from "../native/teleport";
import { createTeleportAppsBridge, type TeleportAppsBridge } from "../native/teleportApps";
import {
  gridPrimary,
  gridStep,
  gridTabs,
  remoteGrid,
  windowGrid,
  type PickerGridTab,
} from "../model/teleportFlow";
import { AppTeleportPicker } from "./AppTeleportPicker";
import { useGridLoads } from "./pickerLoads";
import { PickerTileView, tileIconKey, tileThumbnailKey } from "./PickerTile";
import { SearchGlyph } from "./SearchGlyph";

/** The picker's tabs: the app catalog (default), this Mac's open windows,
 * and the Space's own windows (streamed here). */
type Tab = "apps" | PickerTab;

/**
 * The teleport picker window (label `teleport-picker`): an ordinary,
 * decorated window. Its default tab is "Teleport an app…" over the cua SDK
 * (every app on this Mac, what can move, consent, progress). "Open windows"
 * picks an app by one of its windows; "From {Space}" streams one of the
 * Space's windows here. It reads its target Space (and an optional
 * preselected app from a drop or a window drag) from the shell.
 */
export function TeleportPicker({
  bridge,
  apps,
}: { bridge?: TeleportBridge; apps?: TeleportAppsBridge } = {}) {
  const teleport = useMemo(() => bridge ?? createTeleportBridge(), [bridge]);
  const appsBridge = useMemo(() => apps ?? createTeleportAppsBridge(), [apps]);
  const [config, setConfig] = useState<TeleportPickerConfig | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);
  const [tab, setTab] = useState<Tab>("apps");
  const [preselect, setPreselect] = useState<{ entry: CatalogEntry; files?: string[] } | null>(null);
  // Remounts the app picker for a new target or preselection.
  const [generation, setGeneration] = useState(0);
  // This Mac's windows, front to back: the Apps tiles preview their app's
  // frontmost one.
  const [openWindows, setOpenWindows] = useState<OpenWindow[] | null>(null);
  useEffect(() => {
    let cancelled = false;
    teleport
      .listOpenWindows()
      .then((list) => !cancelled && setOpenWindows(list))
      .catch(() => !cancelled && setOpenWindows([]));
    return () => {
      cancelled = true;
    };
  }, [teleport]);

  // The Space's windows, read in the background as soon as the target is
  // known, so "From <Space>" is ready when chosen.
  const spaceId = config?.spaceId ?? null;
  const [remoteWindows, setRemoteWindows] = useState<RemoteWindow[] | null>(null);
  useEffect(() => {
    setRemoteWindows(null);
    if (!spaceId) return;
    let cancelled = false;
    teleport
      .listRemoteWindows(spaceId)
      .then((list) => !cancelled && setRemoteWindows(list))
      .catch(() => !cancelled && setRemoteWindows([]));
    return () => {
      cancelled = true;
    };
  }, [teleport, spaceId]);

  const close = useCallback(() => {
    void teleport.closePicker().catch(() => {});
  }, [teleport]);

  const initFrom = useCallback((c: TeleportPickerConfig) => {
    setConfig(c);
    setTab("apps");
    setPreselect(c.entry ? { entry: entryFromCore(c.entry), files: c.files ?? [] } : null);
    setGeneration((g) => g + 1);
  }, []);

  // Load the target on mount, and re-init when the shell re-targets the window.
  useEffect(() => {
    let cancelled = false;
    teleport
      .pickerConfig()
      .then((c) => {
        if (!cancelled) initFrom(c);
      })
      .catch((error: unknown) => {
        if (!cancelled) setFatal(describe(error));
      });

    let unlisten: (() => void) | null = null;
    void import("@tauri-apps/api/event")
      .then(({ listen }) =>
        listen<TeleportPickerConfig>("teleport-picker:retarget", (event) => {
          if (!cancelled) initFrom(event.payload);
        }),
      )
      .then((stop) => {
        if (cancelled) stop();
        else unlisten = stop;
      })
      .catch(() => {});

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [teleport, initFrom]);

  // Escape always cancels the whole flow (closes the window).
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        close();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [close]);

  const host = useMemo(() => (config ? appsBridge.host(config.spaceId) : null), [appsBridge, config]);

  if (fatal) {
    return (
      <div className="hp-window">
        <section className="hp-panel hp-panel-status" role="dialog" aria-modal="true">
          <h1 className="hp-status-title">Could not open the teleport picker</h1>
          <p className="hp-status-error">{fatal}</p>
          <div className="hp-status-actions">
            <button type="button" className="hp-button" onClick={close}>
              Close
            </button>
          </div>
        </section>
      </div>
    );
  }

  if (!config || !host) {
    return (
      <div className="hp-window">
        <section className="hp-panel hp-panel-status" role="dialog" aria-modal="true">
          <p className="hp-status-hint">Preparing…</p>
        </section>
      </div>
    );
  }

  // The tab strip is the core's (the same labels as the SwiftUI app).
  const TAB_IDS: Record<string, Tab> = { apps: "apps", windows: "space", space: "thisMac" };
  const tabs: { id: Tab; label: string }[] = gridTabs(config.spaceName).map((t) => ({
    id: TAB_IDS[t.tab] ?? "apps",
    label: t.label,
  }));

  return (
    <div className="hp-window">
      <section className="hp-panel" role="dialog" aria-modal="true" data-tab={tab}>
        <div className="hp-tabs" role="tablist" aria-label="Teleport">
          {tabs.map((t) => (
            <button
              key={t.id}
              type="button"
              role="tab"
              className="hp-tab"
              data-active={tab === t.id}
              aria-selected={tab === t.id}
              onClick={() => setTab(t.id)}
            >
              {t.label}
            </button>
          ))}
        </div>
        {tab === "apps" ? (
          <AppTeleportPicker
            key={generation}
            host={host}
            spaceName={config.spaceName}
            preselect={preselect}
            autoReview={Boolean(config?.autoReview)}
            onClose={close}
            windows={openWindows}
            captureThumbnail={teleport.captureThumbnail}
          />
        ) : (
          <PickView
            tab={tab}
            initialWindows={openWindows}
            remote={remoteWindows}
            teleport={teleport}
            apps={appsBridge}
            spaceId={config.spaceId}
            spaceName={config.spaceName}
            onChoose={(entry) => {
              setPreselect({ entry });
              setGeneration((g) => g + 1);
              setTab("apps");
            }}
            onCancel={close}
          />
        )}
      </section>
    </div>
  );
}

/**
 * The window tabs, on the core's grid (`teleport::grid`, the same tiles as
 * the Apps tab and the SwiftUI picker):
 *
 *  - Open windows: this machine's windows; choosing one opens its app in
 *    the Apps flow ("Teleport to <Space>").
 *  - From <Space>: the Space's windows (never its screen target); choosing
 *    one streams it here ("Stream to This Mac").
 *
 * Icons and previews load lazily for the tiles shown, through the SDK's
 * caches; the view keeps only what the current grid shows.
 */
function PickView({
  tab,
  initialWindows,
  remote,
  teleport,
  apps,
  spaceId,
  spaceName,
  onChoose,
  onCancel,
}: {
  tab: PickerTab;
  /** This Mac's windows as the picker last read them (shown at once). */
  initialWindows?: OpenWindow[] | null;
  /** The Space's windows (read by the picker when it opened); null while
   * loading. */
  remote: RemoteWindow[] | null;
  teleport: TeleportBridge;
  apps: TeleportAppsBridge;
  spaceId: string;
  spaceName: string;
  onChoose: (entry: CatalogEntry) => void;
  onCancel: () => void;
}) {
  const gridTab: PickerGridTab = tab === "space" ? "windows" : "space";
  const [query, setQuery] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  // This Mac's windows: the picker's list at once, then a fresh one.
  const [windows, setWindows] = useState<OpenWindow[] | null>(initialWindows ?? null);
  const [selected, setSelected] = useState<Record<PickerGridTab, string | null>>({
    apps: null,
    windows: null,
    space: null,
  });
  // Icons and previews: each asked once, a few at a time, in grid order,
  // applied in batches (the SDK caches both).
  const icons = useGridLoads<string>(16);
  const thumbs = useGridLoads<string>(4);

  useEffect(() => {
    let cancelled = false;
    teleport
      .listOpenWindows()
      .then((list) => !cancelled && setWindows(list))
      .catch(() => !cancelled && setWindows([]));
    return () => {
      cancelled = true;
    };
  }, [teleport]);

  useEffect(() => setNotice(null), [tab]);

  const grid = useMemo(
    () =>
      gridTab === "windows"
        ? windowGrid(windows ?? [], query, selected.windows)
        : remoteGrid(remote ?? [], query, selected.space),
    [gridTab, windows, remote, query, selected],
  );
  const loaded = gridTab === "windows" ? windows !== null : remote !== null;
  const tiles = useMemo(() => grid.sections.flatMap((sec) => sec.tiles), [grid]);
  const primary = useMemo(() => gridPrimary(gridTab, spaceName, grid), [gridTab, spaceName, grid]);

  // The shown tiles' icons: this machine's per app path, the Space's in one
  // SDK call (both cached in the SDK).
  useEffect(() => {
    const guests: { key: string; appName: string; appId: string; pid: number }[] = [];
    for (const t of tiles) {
      const key = tileIconKey(t.icon);
      if (!key) continue;
      if (t.icon.kind === "host") {
        const path = t.icon.path;
        icons.request(key, () => apps.hostIcon(path));
      } else if (t.icon.kind === "guest") {
        guests.push({ key, appName: t.icon.appName, appId: t.icon.appId, pid: t.icon.pid });
      }
    }
    if (guests.length === 0) return;
    // One batched call for every new guest icon; each tile reads its own.
    let batch: Promise<(string | null)[]> | null = null;
    const all = () =>
      (batch ??= teleport.spaceAppIcons(
        spaceId,
        guests.map(({ appName, appId, pid }) => ({ appName, appId, pid })),
      ));
    guests.forEach((g, i) => icons.request(g.key, () => all().then((urls) => urls[i] ?? null)));
  }, [tiles, apps, teleport, spaceId, icons.request]);

  // Each tile's live preview, lazily (the SDK caches previews briefly).
  useEffect(() => {
    for (const t of tiles) {
      const key = tileThumbnailKey(t.thumbnail);
      if (!key) continue;
      const thumb = t.thumbnail;
      thumbs.request(key, () =>
        thumb.kind === "host-window"
          ? teleport.captureThumbnail(thumb.windowId)
          : thumb.kind === "guest-window"
            ? teleport.remoteWindowThumbnail(spaceId, thumb.windowId, thumb.epoch)
            : Promise.resolve(null),
      );
    }
  }, [tiles, teleport, spaceId, thumbs.request]);

  const select = (id: string) => setSelected((s) => ({ ...s, [gridTab]: id }));

  const activate = (id: string, replica = false) => {
    select(id);
    if (gridTab === "windows") {
      const win = windows?.find((w) => String(w.windowId) === id);
      if (!win) return;
      if (win.entry) {
        onChoose(entryFromCore(win.entry));
        return;
      }
      if (!win.bundlePath) return;
      apps
        .entryForPath(win.bundlePath)
        .then(onChoose)
        .catch((error: unknown) => setNotice(describe(error)));
      return;
    }
    const win = remote?.find((w) => w.id === id);
    if (!win) return;
    teleport
      .streamRemoteWindow(spaceId, spaceName, win.id, win.appName, win.title, replica)
      .then(() => onCancel()) // close the picker once the stream window is up
      .catch((error: unknown) => setNotice(describe(error)));
  };

  const current = selected[gridTab];
  return (
    <div
      className="hp-pick"
      onKeyDown={(event) => {
        if (event.key === "Enter" && primary.enabled && current) {
          event.preventDefault();
          activate(current);
          return;
        }
        const columns = 3;
        const delta = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: columns, ArrowUp: -columns }[event.key];
        if (delta !== undefined && (event.target as HTMLElement).tagName !== "INPUT") {
          event.preventDefault();
          const next = gridStep(grid, current, delta);
          if (next) select(next);
        }
      }}
    >
      <div className="hp-search">
        <SearchGlyph className="glyph hp-search-glyph" />
        <input
          className="hp-search-input"
          type="search"
          value={query}
          autoFocus
          placeholder="Search"
          aria-label="Search"
          onChange={(event) => setQuery(event.target.value)}
        />
      </div>

      <div className="hp-grid-wrap ta-grid-wrap">
        {grid.sections.map((sec) => (
          <ul key={sec.title} className="hp-grid" role="listbox" aria-label={sec.title || "Windows"}>
            {sec.tiles.map((t) => (
              <PickerTileView
                key={t.id}
                tile={t}
                icon={icons.values.get(tileIconKey(t.icon)) ?? null}
                thumbnail={thumbs.values.get(tileThumbnailKey(t.thumbnail)) ?? null}
                onSelect={() => select(t.id)}
                onActivate={() => activate(t.id)}
              />
            ))}
          </ul>
        ))}
        {loaded && grid.emptyText && <p className="hp-empty">{grid.emptyText}</p>}
      </div>

      <footer className="hp-footer">
        {notice && (
          <p className="hp-notice" role="status">
            {notice}
          </p>
        )}
        <div className="hp-footer-right">
          <button type="button" className="hp-cancel" onClick={onCancel}>
            Cancel
          </button>
          <button
            type="button"
            className="hp-button hp-button-primary"
            disabled={!primary.enabled}
            onClick={() => current && activate(current)}
          >
            {primary.label}
          </button>
        </div>
      </footer>
    </div>
  );
}

function describe(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}
