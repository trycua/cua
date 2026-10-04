// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";

import { type CatalogEntry, MOVE_LABEL, type RunReport, type TeleportHost } from "@trycua/cua/teleport";

import {
  TeleportPickerController,
  appGrid,
  canConfirm,
  canPlan,
  formatBytes,
  gridPrimary,
  gridStep,
  progress,
  sensitiveOptions,
  type PickerTile,
} from "../model/teleportFlow";
import type { OpenWindow } from "../model/teleport";

import { displayPath, displayPaths } from "../model/paths";
import { hasTauri } from "../native/bridge";
import { useGridLoads } from "./pickerLoads";
import { PickerTileView } from "./PickerTile";
import { SearchGlyph } from "./SearchGlyph";

/**
 * "Teleport an app…": every app on this machine, classified by the cua SDK
 * (full: its signed-in state can move; install only: installed in the Space,
 * empty or with chosen files; not available: disabled with the reason), then
 * what moves, the consent screen listing every install, path and secret, and
 * progress. The flow is the SDK's headless `TeleportPickerController`; this
 * is only its view.
 */
export function AppTeleportPicker({
  host,
  spaceName,
  preselect,
  autoReview = false,
  onClose,
  onDone,
  home: homeProp,
  windows,
  captureThumbnail,
}: {
  host: TeleportHost;
  /** This machine's open windows, front to back: each app tile previews its
   * frontmost one. */
  windows?: OpenWindow[] | null;
  /** A window's live preview (the window-drag source, SDK-cached). */
  captureThumbnail?: (windowId: number) => Promise<string | null>;
  spaceName: string;
  /** Open straight on this app (a dropped bundle or a dragged window). */
  preselect?: { entry: CatalogEntry; files?: string[] } | null;
  /** Plan at once and show the consent screen (captures and demos only). */
  autoReview?: boolean;
  onClose: () => void;
  onDone?: (report: RunReport) => void;
  /** The home folder, for `~/...` paths; read from the shell when omitted. */
  home?: string | null;
}) {
  const home = useHome(homeProp);
  const controller = useMemo(
    () =>
      new TeleportPickerController(host, preselect ? { spaceName, preselect } : { spaceName }),
    [host, spaceName, preselect],
  );
  const s = useSyncExternalStore(controller.subscribe, () => controller.state);
  const grid = useMemo(() => appGrid(s, windows ?? []), [s, windows]);
  const tiles = useMemo(() => grid.sections.flatMap((sec) => sec.tiles), [grid]);
  const primary = useMemo(() => gridPrimary("apps", spaceName, grid), [spaceName, grid]);

  useEffect(() => {
    void controller.load();
    if (autoReview && preselect) void controller.plan();
  }, [controller, autoReview, preselect]);

  // The listed apps' icons and previews (the SDK caches both; these hold
  // only what this grid shows): each asked once, a few at a time, in grid
  // order, applied in batches.
  const icons = useGridLoads<string>(16);
  const thumbs = useGridLoads<string>(4);
  useEffect(() => {
    const icon = host.icon;
    if (!icon || !s.entries) return;
    const byId = new Map(s.entries.map((e) => [e.id, e]));
    for (const t of tiles) {
      const e = byId.get(t.id);
      if (e) icons.request(e.id, () => icon(e));
    }
  }, [host, s.entries, tiles, icons.request]);
  useEffect(() => {
    if (!captureThumbnail) return;
    for (const t of tiles) {
      if (t.thumbnail.kind !== "host-window") continue;
      const id = t.thumbnail.windowId;
      thumbs.request(String(id), () => captureThumbnail(id));
    }
  }, [captureThumbnail, tiles, thumbs.request]);

  const reported = useRef(false);
  useEffect(() => {
    if (s.step === "done" && s.report && !reported.current) {
      reported.current = true;
      onDone?.(s.report);
    }
  }, [s.step, s.report, onDone]);

  const tile = (t: PickerTile) => (
    <PickerTileView
      key={t.id}
      tile={t}
      icon={icons.values.get(t.id) ?? null}
      thumbnail={t.thumbnail.kind === "host-window" ? (thumbs.values.get(String(t.thumbnail.windowId)) ?? null) : null}
      onSelect={() => controller.dispatch({ type: "select", id: t.id })}
      onActivate={() => controller.choose(t.id)}
    />
  );

  switch (s.step) {
    case "loading":
      return (
        <Status title="Looking for apps…" spinner />
      );
    case "pick":
      return (
        <div
          className="ta-pick"
          onKeyDown={(ev) => {
            if (ev.key === "Enter" && s.selectedId) {
              ev.preventDefault();
              controller.choose();
              return;
            }
            const columns = 3;
            const delta = { ArrowRight: 1, ArrowLeft: -1, ArrowDown: columns, ArrowUp: -columns }[ev.key];
            if (delta !== undefined) {
              ev.preventDefault();
              const next = gridStep(grid, s.selectedId ?? null, delta);
              if (next) controller.dispatch({ type: "select", id: next });
            }
          }}
        >
          <div className="hp-search">
            <SearchGlyph className="glyph hp-search-glyph" />
            <input
              className="hp-search-input"
              type="search"
              value={s.query}
              autoFocus
              placeholder="Search apps"
              aria-label="Search apps"
              onChange={(ev) => controller.dispatch({ type: "query", query: ev.target.value })}
            />
          </div>
          <div className="hp-grid-wrap ta-grid-wrap">
            {grid.sections.map((sec) => (
              <section key={sec.title} className="ta-section" aria-label={sec.title}>
                <h2 className="ta-section-title">{sec.title}</h2>
                <ul className="hp-grid" role="listbox" aria-label={sec.title}>
                  {sec.tiles.map(tile)}
                </ul>
              </section>
            ))}
            {grid.emptyText && <p className="hp-empty">{grid.emptyText}</p>}
          </div>
          <footer className="hp-footer">
            <div className="hp-footer-right">
              <button type="button" className="hp-cancel" onClick={onClose}>
                Cancel
              </button>
              <button
                type="button"
                className="hp-button hp-button-primary"
                disabled={!primary.enabled}
                onClick={() => controller.choose()}
              >
                {primary.label}
              </button>
            </div>
          </footer>
        </div>
      );
    case "options": {
      const e = s.entry!;
      return (
        <div className="ta-options">
          <header className="ta-head">
            {icons.values.get(e.id) ? (
              <img className="ta-icon" src={icons.values.get(e.id) ?? ""} alt="" />
            ) : (
              <span className="ta-icon" aria-hidden="true" />
            )}
            <h1 className="hp-status-title">{e.name}</h1>
          </header>
          <div className="ta-choices" role="radiogroup" aria-label="What moves">
            {e.moves.map((m) => (
              <label key={m} className="ta-choice">
                <input
                  type="radio"
                  name="move"
                  checked={s.move === m}
                  onChange={() => controller.dispatch({ type: "move", move: m })}
                />
                <span>{MOVE_LABEL[m]}</span>
              </label>
            ))}
          </div>
          {sensitiveOptions(s).length > 0 && (
            <div className="ta-choices" aria-label="Also send">
              {sensitiveOptions(s).map((o) => (
                <label key={o.group} className="ta-choice ta-optin">
                  <input
                    type="checkbox"
                    checked={o.checked}
                    onChange={(ev) => controller.dispatch({ type: "sensitive", group: o.group, value: ev.target.checked })}
                  />
                  <span>
                    <span className="ta-optin-label">{o.label}</span>
                    <span className="ta-optin-detail">{o.detail}</span>
                  </span>
                </label>
              ))}
            </div>
          )}
          {s.move === "app_with_files" && (
            <div className="ta-files">
              {s.files.map((f) => (
                <div key={f} className="ta-file">
                  <span title={f}>{displayPath(f, home)}</span>
                  <button type="button" className="hp-cancel" onClick={() => controller.dispatch({ type: "remove-file", path: f })}>
                    Remove
                  </button>
                </div>
              ))}
              {host.chooseFiles ? (
                <button type="button" className="hp-button" onClick={() => void controller.chooseFiles()}>
                  Add files or folders…
                </button>
              ) : null}
            </div>
          )}
          {e.reason && <p className="hp-note">{e.reason}</p>}
          <footer className="hp-footer">
            {s.entries ? (
              <button type="button" className="hp-cancel" onClick={() => controller.dispatch({ type: "back" })}>
                Back
              </button>
            ) : null}
            <div className="hp-footer-right">
              <button type="button" className="hp-cancel" onClick={onClose}>
                Cancel
              </button>
              <button
                type="button"
                className="hp-button hp-button-primary"
                disabled={!canPlan(s)}
                onClick={() => void controller.plan()}
              >
                Review
              </button>
            </div>
          </footer>
        </div>
      );
    }
    case "planning":
      return <Status title={`Preparing ${s.entry?.name ?? "the teleport"}…`} spinner />;
    case "consent": {
      const p = s.plan!;
      return (
        <div className="hp-consent ta-consent">
          <header className="hp-consent-head">
            <h1 className="hp-consent-title">
              Teleport {p.app.name} to {spaceName}?
            </h1>
          </header>
          <ul className="hp-items" role="list" aria-label="What moves">
            {p.consent.map((c) => (
              <li key={`${c.kind}:${c.key}`} className="hp-item" data-sensitive={c.sensitive} data-kind={c.kind}>
                <span className="hp-item-label" title={displayPaths(c.detail, home)}>
                  {displayPaths(c.label, home)}
                </span>
                {c.sensitive && <span className="hp-flag">Secret</span>}
                <span className="hp-item-meta">{c.bytes ? formatBytes(c.bytes) : ""}</span>
              </li>
            ))}
          </ul>
          {p.warnings.map((w) => (
            <p key={w} className="hp-note">
              {w}
            </p>
          ))}
          {p.sensitive && (
            <label className="ta-choice ta-ack">
              <input
                type="checkbox"
                checked={s.acknowledged}
                onChange={(ev) => controller.dispatch({ type: "acknowledge", value: ev.target.checked })}
              />
              <span>Send the secrets above</span>
            </label>
          )}
          {p.sensitive && (
            <label className="ta-choice ta-save-to-keyvault" title="Keep this signed-in session sealed in your Cua Keyvault so it can be delivered again without asking again.">
              <input
                type="checkbox"
                checked={s.saveToKeyvault}
                onChange={(ev) => controller.dispatch({ type: "save-to-keyvault", value: ev.target.checked })}
              />
              <span>Save to Keyvault for reuse</span>
            </label>
          )}
          {p.relayUnsealed && (
            <label className="ta-choice ta-ack">
              <input
                type="checkbox"
                checked={s.acknowledgedRelayPlaintext}
                onChange={(ev) =>
                  controller.dispatch({ type: "acknowledge-relay-plaintext", value: ev.target.checked })
                }
              />
              <span>Send without end-to-end encryption</span>
            </label>
          )}
          <footer className="hp-footer">
            <button type="button" className="hp-cancel" onClick={() => controller.dispatch({ type: "back" })}>
              Back
            </button>
            <span className="hp-footer-note">
              {p.totalBytes ? `${formatBytes(p.totalBytes)} leaves this Mac` : "Nothing leaves this Mac"}
            </span>
            <div className="hp-footer-right">
              <button type="button" className="hp-cancel" onClick={onClose}>
                Cancel
              </button>
              <button
                type="button"
                className="hp-button hp-button-primary"
                disabled={!canConfirm(s)}
                onClick={() => void controller.confirm()}
              >
                Teleport
              </button>
            </div>
          </footer>
        </div>
      );
    }
    case "running": {
      const last = s.events[s.events.length - 1];
      const pct = Math.round(progress(s) * 100);
      return (
        <div className="hp-status">
          <h1 className="hp-status-title">
            Teleporting {s.plan?.app.name} to {spaceName}…
          </h1>
          <div className="ta-bar" role="progressbar" aria-valuenow={pct} aria-valuemin={0} aria-valuemax={100}>
            <span style={{ width: `${pct}%` }} />
          </div>
          <p className="hp-status-hint">{last ? `${last.kind}: ${last.detail}` : "Starting…"}</p>
        </div>
      );
    }
    case "done": {
      const r = s.report!;
      const parts = [
        r.installed.length ? `Installed ${r.installed.join(", ")}.` : "",
        r.sent.length ? `Sent ${r.sent.length} item(s).` : "",
        r.imported.length ? `Imported ${r.imported.length} item(s).` : "",
        r.launched ? "Opened." : "",
      ].filter(Boolean);
      return (
        <Status
          title={`${s.plan?.app.name ?? "The app"} is in ${spaceName}`}
          hint={parts.join(" ")}
          primary={{ label: "Done", onClick: onClose }}
        />
      );
    }
    case "error":
      return (
        <Status
          title={`Could not teleport${s.entry ? ` ${s.entry.name}` : ""}`}
          error={s.error ?? ""}
          secondary={{ label: "Back", onClick: () => controller.dispatch({ type: "back" }) }}
          primary={{ label: "Close", onClick: onClose }}
        />
      );
  }
}

function Status({
  title,
  hint,
  error,
  spinner,
  primary,
  secondary,
}: {
  title: string;
  hint?: string;
  error?: string;
  spinner?: boolean;
  primary?: { label: string; onClick: () => void };
  secondary?: { label: string; onClick: () => void };
}) {
  return (
    <div className="hp-status" role={spinner ? "status" : undefined}>
      <h1 className="hp-status-title">{title}</h1>
      {spinner && <span className="hp-spinner" aria-hidden="true" />}
      {hint && <p className="hp-status-hint">{hint}</p>}
      {error && (
        <p className="hp-status-error" role="alert">
          {error}
        </p>
      )}
      {(primary || secondary) && (
        <div className="hp-status-actions">
          {secondary && (
            <button type="button" className="hp-cancel" onClick={secondary.onClick}>
              {secondary.label}
            </button>
          )}
          {primary && (
            <button type="button" className="hp-button hp-button-primary" onClick={primary.onClick}>
              {primary.label}
            </button>
          )}
        </div>
      )}
    </div>
  );
}

/** The user's home folder: the prop when given (tests), else the shell's. */
function useHome(given: string | null | undefined): string | null {
  const [home, setHome] = useState<string | null>(given ?? null);
  useEffect(() => {
    if (given !== undefined) {
      setHome(given);
      return;
    }
    if (!hasTauri()) return;
    let cancelled = false;
    void import("@tauri-apps/api/path")
      .then(({ homeDir }) => homeDir())
      .then((h) => !cancelled && setHome(h))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [given]);
  return home;
}
