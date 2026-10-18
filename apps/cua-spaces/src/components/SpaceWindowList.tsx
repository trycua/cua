// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useMemo, useState } from "react";

import type { RemoteWindow } from "../model/teleport";
import type { Space } from "../model/types";
import { pipClick, pipReduce, streamSection, type PipEvent, type StreamRow } from "../model/window";
import type { TeleportBridge } from "../native/teleport";
import { Sym } from "./desktop/Sym";
import { OsIconMark } from "./OsIcon";

interface SpaceWindowListProps {
  space: Pick<Space, "id" | "os" | "osName">;
  teleport: TeleportBridge;
  /** The Desktop row's picture in picture. */
  onPipDesktop: () => void;
  /** Closes the Desktop row's open picture in picture. */
  onClosePipDesktop?: () => void;
  /** A window row's own stream. `replica` is true when the button was
   * shift-clicked: open an ADDITIONAL stream window for the same target (its
   * own media session and participant) instead of focusing the existing one. */
  onPipWindow: (win: RemoteWindow, options: { replica: boolean }) => void;
  /** Filter text from the surrounding search affordance. */
  query?: string;
}

/** Icons are per app; windows of one app share one lookup. */
const iconKey = (appName: string, appId: string) => `${appName.toLowerCase()}\u001f${appId.toLowerCase()}`;

/**
 * A Space's Stream section: the rows are the core's (`spaces::stream`, the
 * same ones the SwiftUI app draws). The Desktop row carries the Space's OS
 * mark and the primary display's resolution; a window row carries its app's
 * icon (the SDK's `Space.app_icon`, none when the Space has none), its title
 * and a picture-in-picture button.
 */
export function SpaceWindowList({
  space,
  teleport,
  onPipDesktop,
  onClosePipDesktop,
  onPipWindow,
  query = "",
}: SpaceWindowListProps) {
  const spaceId = space.id;
  const [windows, setWindows] = useState<RemoteWindow[] | null>(null);
  const [failed, setFailed] = useState(false);
  const [display, setDisplay] = useState<{ widthPx: number; heightPx: number } | null>(null);
  // The current rows' icons (from the SDK's one icon cache; replaced on
  // every load, never kept across loads).
  const [icons, setIcons] = useState<Map<string, string>>(() => new Map());

  useEffect(() => {
    let cancelled = false;
    setWindows(null);
    setFailed(false);
    setDisplay(null);
    setIcons(new Map());
    teleport
      .listRemoteWindows(spaceId)
      .then((list) => {
        if (!cancelled) setWindows(list);
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        console.warn(`[Cua Spaces] listing ${spaceId}'s windows failed`, error);
        setWindows([]);
        setFailed(true);
      });
    teleport
      .spacePrimaryDisplay(spaceId)
      .then((d) => {
        if (!cancelled) setDisplay(d);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [teleport, spaceId]);

  // The open picture-in-picture panels (row ids), as the core tracks them
  // from what the shell reports: the rows show pip.exit while theirs is open.
  const [open, setOpen] = useState<string[]>([]);
  const report = useCallback((event: PipEvent) => setOpen((o) => pipReduce(o, event)), []);
  useEffect(() => {
    let cancelled = false;
    setOpen([]);
    const sync = () =>
      void teleport
        .streamPanels?.(spaceId)
        .then((rows) => !cancelled && report({ type: "synced", rows }))
        .catch(() => {});
    sync();
    let stop: (() => void) | null = null;
    void teleport
      .onStreamPanelsChanged?.(sync)
      .then((unlisten) => {
        if (cancelled) unlisten();
        else stop = unlisten;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      stop?.();
    };
  }, [teleport, spaceId, report]);

  const section = useMemo(
    () => streamSection({ windows, failed, display, os: space.os, osName: space.osName ?? null, open, query }),
    [windows, failed, display, space.os, space.osName, open, query],
  );

  const byId = useMemo(() => new Map((windows ?? []).map((w) => [w.id, w])), [windows]);

  const click = (row: StreamRow, replica: boolean) => {
    const win = byId.get(row.id);
    // A shift-click always opens another stream of the window.
    const command = replica && win ? { type: "open" as const, row: row.id } : pipClick(open, row.id);
    if (command.type === "close") {
      report({ type: "closed", row: row.id });
      if (row.kind === "desktop") onClosePipDesktop?.();
      else void teleport.closeStreamWindow?.(spaceId, row.id).catch(() => {});
      return;
    }
    if (row.kind === "desktop") onPipDesktop();
    else if (win) onPipWindow(win, { replica });
    else return;
    report({ type: "opened", row: row.id });
  };

  // Every row's app icon in one SDK call (its cache answers repeats).
  const iconRequests = useMemo(() => {
    const seen = new Map<string, { appName: string; appId: string; pid: number }>();
    for (const row of section.rows) {
      if (row.icon.kind !== "app") continue;
      const key = iconKey(row.icon.appName, row.icon.appId);
      if (!seen.has(key)) seen.set(key, { appName: row.icon.appName, appId: row.icon.appId, pid: row.icon.pid });
    }
    return [...seen.entries()];
  }, [section]);
  useEffect(() => {
    if (iconRequests.length === 0) return;
    let cancelled = false;
    teleport
      .spaceAppIcons(
        spaceId,
        iconRequests.map(([, r]) => r),
      )
      .then((urls) => {
        if (cancelled) return;
        const next = new Map<string, string>();
        iconRequests.forEach(([key], i) => {
          const url = urls[i];
          if (url) next.set(key, url);
        });
        setIcons(next);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [iconRequests, teleport, spaceId]);


  const icon = (row: StreamRow) => {
    if (row.icon.kind === "os") return <OsIconMark id={row.icon.id} size={16} className="ssr-os" />;
    const url = icons.get(iconKey(row.icon.appName, row.icon.appId));
    // No icon for the app: nothing, never a stand-in glyph.
    return url ? <img className="ssr-app-icon" src={url} alt="" data-testid="ssr-app-icon" /> : null;
  };

  return (
    <div className="ssr">
      <ul className="ssr-rows" aria-label="Stream">
        {section.rows.map((row) => (
          <li className="ssr-row" key={row.id} data-kind={row.kind}>
            <span className="ssr-icon" aria-hidden="true">
              {icon(row)}
            </span>
            <span className="ssr-label" title={row.help}>
              {row.label}
            </span>
            {row.actions.map((action) => (
              <button
                key={action.id}
                type="button"
                className="dw-icon-btn ssr-action"
                aria-label={action.help}
                title={action.help}
                aria-pressed={action.active}
                onClick={(event) => click(row, event.shiftKey)}
              >
                <Sym name={action.symbol} size={15} />
              </button>
            ))}
          </li>
        ))}
      </ul>
      {section.statusText && (
        <p className="ssr-status" role="status">
          {section.statusText}
        </p>
      )}
    </div>
  );
}
