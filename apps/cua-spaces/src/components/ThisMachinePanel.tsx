// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useState } from "react";

import { hostPanel, settingChange, type HostAction, type HostActionId, type PermissionRow } from "../model/host";
import type { HostBridge, HostStatus } from "../native/host";
import { HostSetupForm } from "./HostSetupForm";

/** "just now", "5 min ago", "3 h ago", "2 d ago" (the viewer's locale). */
export function relativeTime(atMs: number, nowMs: number): string {
  const s = Math.max(0, Math.round((nowMs - atMs) / 1000));
  const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "auto", style: "short" });
  if (s < 60) return rtf.format(0, "second");
  if (s < 3600) return rtf.format(-Math.floor(s / 60), "minute");
  if (s < 86400) return rtf.format(-Math.floor(s / 3600), "hour");
  return rtf.format(-Math.floor(s / 86400), "day");
}

/** Panes host setup still needs; the user grants them in System Settings. */
export function PermissionRows({
  title,
  rows,
  openLabel,
  onOpen,
}: {
  title: string | null;
  rows: PermissionRow[];
  openLabel: string;
  onOpen: (url: string) => void;
}) {
  if (rows.length === 0) return null;
  return (
    <div className="host-permissions" role="group" aria-label={title ?? undefined}>
      {title && <h3>{title}</h3>}
      <ul>
        {rows.map((p) => (
          <li key={p.id}>
            <span title={p.help || undefined}>{p.title}</span>
            {p.settingsUrl && (
              <button type="button" className="link-button" data-owns-enter onClick={() => onOpen(p.settingsUrl!)}>
                {openLabel}
              </button>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}

/**
 * The "This machine" roster entry's page: set up for access (the host flow),
 * or, once configured, how it is shared, who is connected, and the Stop
 * sharing kill switch. What shows is the app core's `host::panel`.
 */
export function ThisMachinePanel({
  host,
  identity,
  onStatus,
  initialSetup = false,
}: {
  host: HostBridge;
  identity?: string;
  /** Reports every status change (the sidebar row follows it). */
  onStatus?: (status: HostStatus) => void;
  /** Open with the setup form showing (captures). */
  initialSetup?: boolean;
}) {
  const [status, setStatus] = useState<HostStatus | null>(null);
  const [setup, setSetup] = useState(initialSetup);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<HostAction | null>(null);

  const apply = useCallback(
    (next: HostStatus) => {
      setStatus(next);
      onStatus?.(next);
    },
    [onStatus],
  );

  useEffect(() => {
    let cancelled = false;
    void host
      .status()
      .then((s) => {
        if (!cancelled) apply(s);
      })
      .catch((err: unknown) => {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      cancelled = true;
    };
  }, [host, apply]);

  const act = (run: () => Promise<HostStatus | void>) => {
    setBusy(true);
    setError(null);
    run()
      .then((next) => {
        if (next) apply(next);
      })
      .catch((err: unknown) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setBusy(false));
  };

  if (setup) {
    return (
      <HostSetupForm
        identity={identity}
        showTitle
        onSetup={(request) => host.setup(request)}
        onBack={() => setSetup(false)}
        onDone={(next) => {
          apply(next);
          setSetup(false);
        }}
      />
    );
  }

  const view = hostPanel(status);
  const run = (id: HostActionId) => {
    const change = settingChange(id);
    if (change) {
      act(() => host.configure(change));
      return;
    }
    switch (id) {
      case "set-up":
        setSetup(true);
        break;
      case "stop-sharing":
        act(() => host.stopSharing());
        break;
      case "resume-sharing":
        act(() => host.startSharing());
        break;
      case "remove":
        act(async () => {
          await host.remove();
          return host.status();
        });
        break;
    }
  };

  return (
    <div className="this-machine" aria-label={view.title} aria-busy={status ? undefined : true}>
      <p className="this-machine-summary" role="status">
        {view.summary}
      </p>
      {view.facts.length > 0 && (
        <dl className="this-machine-facts">
          {view.facts.map((f) => (
            <div key={f.label}>
              <dt>{f.label}</dt>
              <dd>{f.value}</dd>
            </div>
          ))}
        </dl>
      )}
      {view.toggles && view.toggles.length > 0 && (
        <div className="this-machine-settings" role="group" aria-label="Sharing">
          {view.toggles.map((t) => (
            <label className="create-cloud-toggle" key={t.id} title={t.help}>
              <input
                type="checkbox"
                role="switch"
                checked={t.on}
                disabled={busy || !t.enabled}
                aria-describedby={`host-toggle-${t.id}`}
                onChange={() => run(t.action)}
              />
              <span>{t.label}</span>
              <span id={`host-toggle-${t.id}`} className="this-machine-muted">
                {t.help}
              </span>
            </label>
          ))}
          {view.limits && <p className="this-machine-muted">{view.limits}</p>}
        </div>
      )}
      {view.providedTitle && (
        <div className="this-machine-clients" aria-label={view.providedTitle}>
          <h3>{view.providedTitle}</h3>
          {view.providedEmpty ? (
            <p className="this-machine-muted">{view.providedEmpty}</p>
          ) : (
            <ul>
              {(view.provided ?? []).map((r) => (
                <li key={`${r.atMs}-${r.text}`}>
                  {r.text} <span className="this-machine-muted">{relativeTime(r.atMs, Date.now())}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
      {view.accessWarning && (
        <p className="create-cloud-error" role="alert">
          {view.accessWarning}
        </p>
      )}
      {view.clientsTitle && (
        <div className="this-machine-clients" aria-label={view.clientsTitle}>
          <h3>{view.clientsTitle}</h3>
          {view.clientsEmpty ? (
            <p className="this-machine-muted">{view.clientsEmpty}</p>
          ) : (
            <ul>
              {view.clients.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          )}
        </div>
      )}
      {view.recentTitle && view.recent && view.recent.length > 0 && (
        <div className="this-machine-clients" aria-label={view.recentTitle}>
          <h3>{view.recentTitle}</h3>
          <ul>
            {view.recent.map((r) => (
              <li key={`${r.atMs}-${r.text}`}>
                {r.text} <span className="this-machine-muted">{relativeTime(r.atMs, Date.now())}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
      {view.activityWarning && (
        <p className="create-cloud-error" role="alert">
          {view.activityWarning}
        </p>
      )}
      {view.activityTitle && view.activity && view.activity.length > 0 && (
        <div className="this-machine-clients" aria-label={view.activityTitle}>
          <h3>{view.activityTitle}</h3>
          <ul>
            {view.activity.map((r) => (
              <li key={`${r.atMs}-${r.text}`}>
                {r.text} <span className="this-machine-muted">{relativeTime(r.atMs, Date.now())}</span>
              </li>
            ))}
          </ul>
        </div>
      )}
      <PermissionRows
        title={view.permissionsTitle}
        rows={view.permissions}
        openLabel={view.openSettingsLabel}
        onOpen={(url) => void host.openSettings(url)}
      />
      {error && (
        <p className="create-cloud-error" role="alert">
          {error}
        </p>
      )}
      {view.actions.length > 0 && (
        <div className="this-machine-actions">
          {view.actions.map((a, i) => (
            <button
              key={a.id}
              type="button"
              className={`${i === 0 ? "primary-button" : "secondary-button"}${a.destructive ? " danger" : ""}`}
              data-owns-enter
              disabled={busy}
              onClick={() => (a.confirm ? setConfirming(a) : run(a.id))}
            >
              {a.label}
            </button>
          ))}
        </div>
      )}
      {confirming?.confirm && (
        <div className="dw-banner" role="alertdialog" aria-label={confirming.confirm.title}>
          <span>
            <strong>{confirming.confirm.title}</strong> {confirming.confirm.message}
          </span>
          <button type="button" className="dw-btn" onClick={() => setConfirming(null)}>
            {confirming.confirm.cancelLabel}
          </button>
          <button
            type="button"
            className="dw-btn dw-btn-danger"
            disabled={busy}
            onClick={() => {
              const id = confirming.id;
              setConfirming(null);
              run(id);
            }}
          >
            {confirming.confirm.confirmLabel}
          </button>
        </div>
      )}
    </div>
  );
}
