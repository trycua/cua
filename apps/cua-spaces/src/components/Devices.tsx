// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";

import {
  approveOpen,
  approveView,
  cleanDeviceName,
  devicesView,
  enrollInitial,
  enrollView,
  reduceApprove,
  reduceEnroll,
  type ApprovalPrompt,
  type ApproveSheetAction,
  type DeviceInput,
  type DeviceRow,
  type DevicesInput,
  type DevicesView,
  type EnrollAction,
  type UnconfirmedMachine,
} from "../model/devices";
import type { DevicesBridge } from "../native/devices";
import { enrollSignals } from "../model/telemetry";
import { telemetryBridge } from "../native/telemetry";
import { relativeTime } from "./ThisMachinePanel";

/** How often the page (and the approval watch) reads the relay. */
export const DEVICES_POLL_MS = 60_000;
/** How often the enroll sheet asks whether an approval arrived. */
export const ENROLL_POLL_MS = 3_000;
/** How many times it asks (the code expires after 10 minutes). */
export const ENROLL_POLL_LIMIT = 200;

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** A date as the viewer's locale writes it ("Oct 27, 2026"). */
export function deviceDate(atSecs: number): string {
  return new Date(atSecs * 1000).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

/**
 * The Devices page's data, read quietly while signed in (every
 * [`DEVICES_POLL_MS`]), and the approval watch: a device that asks for the
 * first time this session posts a notification and opens the sheet. No
 * other prompt: agents on autopilot never see one.
 */
export function useDevices(bridge: DevicesBridge, signedIn: boolean, nowMs: () => number) {
  const [input, setInput] = useState<DevicesInput | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [approving, setApproving] = useState<ApprovalPrompt | null>(null);
  const seen = useRef(new Set<string>());
  const active = bridge.isNative && signedIn;

  const refresh = useCallback(async () => {
    if (!active) return;
    try {
      setInput(await bridge.snapshot());
      setError(null);
    } catch (e) {
      setError(message(e));
    }
  }, [active, bridge]);

  useEffect(() => {
    if (!active) {
      setInput(null);
      return;
    }
    void refresh();
    const timer = window.setInterval(() => void refresh(), DEVICES_POLL_MS);
    return () => window.clearInterval(timer);
  }, [active, refresh]);

  const view = useMemo(() => (input ? devicesView(input, nowMs() / 1000) : null), [input, nowMs]);

  useEffect(() => {
    if (!view) return;
    for (const prompt of view.approvals) {
      if (seen.current.has(prompt.deviceId)) continue;
      seen.current.add(prompt.deviceId);
      void bridge.notify(prompt.notifyTitle, prompt.notifyBody).catch(() => {});
      setApproving((current) => current ?? prompt);
    }
  }, [bridge, view]);

  /** Marks a device already decided, like closing its sheet: no more auto-prompt or notification this session. */
  const dismiss = useCallback((deviceId: string) => {
    seen.current.add(deviceId);
  }, []);

  return { input, view, error, refresh, approving, setApproving, dismiss };
}

/** The main window's slim banner (enrollment needed, re-verification soon). */
export function DevicesBanner({ view, onEnroll }: { view: DevicesView | null; onEnroll: () => void }) {
  const banner = view?.banner;
  if (!banner) return null;
  return (
    <div className="dw-banner dv-banner" data-tone={banner.tone} role="status" aria-label="Device enrollment">
      <span>{banner.text}</span>
      {banner.action === "enroll" && banner.actionLabel && (
        <button type="button" className="dw-btn" onClick={onEnroll}>
          {banner.actionLabel}
        </button>
      )}
    </div>
  );
}

/** Settings → Devices: this device, the account's devices, recent access. */
export function DevicesSection({
  view,
  error,
  signedIn,
  nowMs,
  onEnroll,
  onApprove,
  onDeny,
  onRename,
  onRevoke,
  onConfirmMachine,
}: {
  view: DevicesView | null;
  error: string | null;
  signedIn: boolean;
  nowMs: number;
  onEnroll: () => void;
  onApprove: (prompt: ApprovalPrompt) => void;
  onDeny: (prompt: ApprovalPrompt) => void;
  onRename: (row: DeviceRow) => void;
  onRevoke: (row: DeviceRow) => void;
  onConfirmMachine: (machine: UnconfirmedMachine) => void;
}) {
  if (!signedIn || !view) {
    return (
      <section className="st-group dv" aria-label="Devices">
        <div className="st-head">
          <h3 className="st-title">Devices</h3>
        </div>
        <div className="st-rows">
          <p className="st-note">{signedIn ? (error ?? "Loading…") : "Sign in to Cua to see the devices that can reach your machines."}</p>
        </div>
      </section>
    );
  }
  const l = view.labels;
  const t = view.thisDevice;
  return (
    <>
      <section className="st-group dv" aria-label={l.thisDevice}>
        <div className="st-head">
          <h3 className="st-title">{l.thisDevice}</h3>
        </div>
        <div className="st-rows">
          <div className="st-row" data-kind={t.kind}>
            <span className="st-label">{t.name ?? l.thisDevice}</span>
            <span className="st-value" data-testid="this-device-status">
              {t.at != null ? `${t.title} ${deviceDate(t.at)}` : t.title}
            </span>
            {t.actionLabel && (
              <button type="button" className="dw-btn" data-owns-enter onClick={onEnroll}>
                {t.actionLabel}
              </button>
            )}
          </div>
        </div>
        {error && (
          <p className="st-error" role="alert">
            {error}
          </p>
        )}
      </section>

      {view.unconfirmedMachines.length > 0 && (
        <section className="st-group dv" aria-label={l.newMachines}>
          <div className="st-head">
            <h3 className="st-title">{l.newMachines}</h3>
          </div>
          <div className="st-rows">
            {view.unconfirmedMachines.map((m) => (
              <div className="st-row dv-row" key={m.id}>
                <span className="st-label">{m.title}</span>
                <button type="button" className="dw-btn" data-owns-enter onClick={() => onConfirmMachine(m)}>
                  {l.confirmMachine}
                </button>
              </div>
            ))}
          </div>
        </section>
      )}

      <section className="st-group dv" aria-label={l.devices}>
        <div className="st-head">
          <h3 className="st-title">{l.devices}</h3>
        </div>
        <div className="st-rows">
          {view.rows.map((row) => {
            const prompt = view.approvals.find((a) => a.deviceId === row.id);
            return (
              <div className="st-row dv-row" key={row.id} data-current={row.current || undefined}>
                <div className="dv-text">
                  <span className="st-label">{row.title}</span>
                  <span className="dv-detail">
                    {row.detail}
                    {row.lastSeen != null && ` · ${l.lastSeen} ${relativeTime(row.lastSeen * 1000, nowMs)}`}
                  </span>
                </div>
                {row.actions.includes("approve") && prompt && (
                  <>
                    <button type="button" className="dw-btn" data-owns-enter onClick={() => onApprove(prompt)}>
                      {l.approve}
                    </button>
                    <button type="button" className="dw-btn" onClick={() => onDeny(prompt)}>
                      {l.deny}
                    </button>
                  </>
                )}
                {row.actions.includes("rename") && (
                  <button type="button" className="dw-btn" data-owns-enter onClick={() => onRename(row)}>
                    {l.rename}
                  </button>
                )}
                {row.actions.includes("revoke") && (
                  <button type="button" className="dw-btn" data-owns-enter onClick={() => onRevoke(row)}>
                    {l.revoke}
                  </button>
                )}
              </div>
            );
          })}
        </div>
      </section>

      <section className="st-group dv" aria-label={l.recent}>
        <div className="st-head">
          <h3 className="st-title">{l.recent}</h3>
        </div>
        <div className="st-rows">
          {view.recent.length === 0 ? (
            <p className="st-note">{l.recentEmpty}</p>
          ) : (
            view.recent.map((a, i) => (
              <div className="st-row" key={`${a.ts}-${i}`} data-notable={a.notable || undefined}>
                <span className="st-label" title={a.text}>
                  {a.notable && <span className="dv-notable" aria-label="Not one of your devices" />}
                  {a.text}
                </span>
                <span className="st-value">{relativeTime(a.ts * 1000, nowMs)}</span>
              </div>
            ))
          )}
        </div>
      </section>
    </>
  );
}

/** A small sheet over the window (Esc and a click outside close it). */
export function SmallSheet({ label, onClose, children }: { label: string; onClose: () => void; children: ReactNode }) {
  return (
    <div
      className="dw-scrim"
      onMouseDown={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
      onKeyDown={(event) => {
        if (event.key === "Escape") onClose();
        event.stopPropagation();
      }}
    >
      <div className="dw-sheet dv-sheet" role="dialog" aria-label={label}>
        {children}
      </div>
    </div>
  );
}

/**
 * "Enroll This Device": the core's enroll sheet. Sign in again runs the
 * app's interactive sign-in, then registers; Approve from another device
 * registers and shows the code, then waits for the approval.
 */
export function EnrollSheet({
  bridge,
  signIn,
  onClose,
  onEnrolled,
  pollMs = ENROLL_POLL_MS,
}: {
  bridge: DevicesBridge;
  /** The app's interactive sign-in; resolves once signed in. */
  signIn: () => Promise<void>;
  onClose: () => void;
  onEnrolled: () => void;
  pollMs?: number;
}) {
  const [state, setState] = useState(enrollInitial);
  const stateRef = useRef(state);
  // Every step goes through here: the core's reducer, and the usage events
  // it means (enrolled by a sign-in or an approval, or failed).
  const send = useCallback((action: EnrollAction) => {
    const before = stateRef.current;
    stateRef.current = reduceEnroll(before, action);
    setState(stateRef.current);
    telemetryBridge().recordSignals(enrollSignals(before, action));
  }, []);
  const v = enrollView(state);

  useEffect(() => {
    let cancelled = false;
    const fail = (e: unknown) => !cancelled && send({ type: "failed", error: message(e) });
    if (state.phase === "signing-in") {
      signIn().then(() => !cancelled && send({ type: "signed-in" }), fail);
    } else if (state.phase === "registering") {
      bridge.enroll().then((r) => !cancelled && send({ type: "registered", enrolled: r.enrolled, code: r.code }), fail);
    } else if (state.phase === "waiting") {
      let tries = 0;
      const timer = window.setInterval(() => {
        tries += 1;
        if (tries > ENROLL_POLL_LIMIT) {
          window.clearInterval(timer);
          fail(new Error("The code expired. Go back and start again."));
          return;
        }
        void bridge.checkEnrolled().then((ok) => {
          if (ok && !cancelled) {
            window.clearInterval(timer);
            send({ type: "approved" });
          }
        }, () => {});
      }, pollMs);
      return () => {
        cancelled = true;
        window.clearInterval(timer);
      };
    } else if (state.phase === "enrolled") {
      onEnrolled();
    }
    return () => {
      cancelled = true;
    };
    // Each phase runs its step once.
  }, [state.phase]);

  return (
    <SmallSheet label={v.title} onClose={onClose}>
      <div className="dv-sheet-body">
        <h2 className="dv-sheet-title">{v.title}</h2>
        <p className="dv-sheet-text">{v.lede}</p>
        {v.options.length > 0 && (
          <div className="st-rows dv-options">
            {v.options.map((o) => (
              <button
                key={o.method}
                type="button"
                className="dv-option"
                onClick={() => send({ type: "choose", method: o.method })}
              >
                <span className="st-label">{o.title}</span>
                <span className="dv-detail">{o.detail}</span>
              </button>
            ))}
          </div>
        )}
        {v.code && (
          <p className="dv-code" aria-label="One-time code">
            {v.code}
          </p>
        )}
        {v.codeHelp && <p className="dv-sheet-text">{v.codeHelp}</p>}
        {v.status && (
          <p className="dv-sheet-text" role="status">
            {v.status}
          </p>
        )}
        {v.error && (
          <p className="st-error" role="alert">
            {v.error}
          </p>
        )}
      </div>
      <div className="dv-sheet-actions">
        {v.backLabel && (
          <button type="button" className="dw-btn" onClick={() => send({ type: "back" })}>
            {v.backLabel}
          </button>
        )}
        <button type="button" className={v.done ? "dw-btn dw-btn-primary" : "dw-btn"} onClick={onClose}>
          {v.closeLabel}
        </button>
      </div>
    </SmallSheet>
  );
}

/**
 * The approval sheet (another device asks): the code it shows, Approve
 * (presence in the shell, then the relay), Deny in one click.
 */
export function ApproveSheet({
  bridge,
  prompt,
  devices = [],
  onClose,
  onDone,
}: {
  bridge: DevicesBridge;
  prompt: ApprovalPrompt;
  /** The account's current devices (the code-expired fallback by id). */
  devices?: DeviceInput[];
  onClose: () => void;
  onDone: () => void;
}) {
  const [state, setState] = useState(() => approveOpen(prompt));
  const send = useCallback(
    (action: ApproveSheetAction) => setState((s) => reduceApprove(s, action, devices)),
    [devices],
  );
  const [passphrase, setPassphrase] = useState("");
  const [needsPassphrase, setNeedsPassphrase] = useState(false);
  useEffect(() => {
    void bridge.needsPassphrase().then(setNeedsPassphrase, () => {});
  }, [bridge]);
  const v = approveView(state, devices);

  const approve = async () => {
    if (!v.canApprove || !v.request) return;
    send({ type: "submit" });
    try {
      await bridge.approve({ code: v.request.code, deviceId: v.request.deviceId, passphrase: needsPassphrase ? passphrase : null });
      onDone();
    } catch (e) {
      send({ type: "failed", error: message(e) });
    }
  };

  const deny = async () => {
    if (v.denyRevokes) {
      try {
        await bridge.revoke(state.deviceId);
      } catch (e) {
        send({ type: "failed", error: message(e) });
        return;
      }
      onDone();
    } else {
      onClose();
    }
  };

  return (
    <SmallSheet label={v.title} onClose={onClose}>
      <form
        className="dv-sheet-body"
        onSubmit={(event) => {
          event.preventDefault();
          void approve();
        }}
      >
        <h2 className="dv-sheet-title">{v.title}</h2>
        <p className="dv-sheet-text">{v.message}</p>
        {v.needsCode && (
          <label className="dw-field">
            <span>{v.codeLabel}</span>
            <input
              className="dw-input dv-code-input"
              value={v.code}
              placeholder={v.codePlaceholder}
              autoFocus
              spellCheck={false}
              autoComplete="off"
              disabled={v.busy}
              onChange={(event) => send({ type: "set-code", code: event.target.value })}
            />
          </label>
        )}
        {needsPassphrase && (
          <label className="dw-field">
            <span>Keyvault passphrase</span>
            <input
              className="dw-input"
              type="password"
              value={passphrase}
              disabled={v.busy}
              onChange={(event) => setPassphrase(event.target.value)}
            />
          </label>
        )}
        {v.error && (
          <p className="st-error" role="alert">
            {v.error}
          </p>
        )}
        <div className="dv-sheet-actions">
          <button type="button" className="dw-btn" disabled={v.busy} onClick={() => void deny()}>
            {v.denyLabel}
          </button>
          <button
            type="submit"
            className="dw-btn dw-btn-primary"
            disabled={!v.canApprove || (needsPassphrase && !passphrase)}
          >
            {v.approveLabel}
          </button>
        </div>
      </form>
    </SmallSheet>
  );
}

/** Rename…: one field, the core cleans the name. */
export function RenameSheet({
  row,
  title,
  confirmLabel,
  cancelLabel,
  onRename,
  onClose,
}: {
  row: DeviceRow;
  title: string;
  confirmLabel: string;
  cancelLabel: string;
  onRename: (name: string) => Promise<void>;
  onClose: () => void;
}) {
  const [name, setName] = useState(row.name);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const cleaned = cleanDeviceName(name);
  return (
    <SmallSheet label={title} onClose={onClose}>
      <form
        className="dv-sheet-body"
        onSubmit={(event) => {
          event.preventDefault();
          if (!cleaned) return;
          setBusy(true);
          onRename(cleaned).then(onClose, (e: unknown) => {
            setBusy(false);
            setError(message(e));
          });
        }}
      >
        <h2 className="dv-sheet-title">{title}</h2>
        <input
          className="dw-input"
          aria-label={title}
          value={name}
          autoFocus
          spellCheck={false}
          onChange={(event) => setName(event.target.value)}
        />
        {error && (
          <p className="st-error" role="alert">
            {error}
          </p>
        )}
        <div className="dv-sheet-actions">
          <button type="button" className="dw-btn" onClick={onClose}>
            {cancelLabel}
          </button>
          <button type="submit" className="dw-btn dw-btn-primary" disabled={!cleaned || busy}>
            {confirmLabel}
          </button>
        </div>
      </form>
    </SmallSheet>
  );
}

/** Confirm…: a new machine's core-owned confirmation (S5). */
export function ConfirmMachineSheet({
  machine,
  onConfirm,
  onClose,
}: {
  machine: UnconfirmedMachine;
  onConfirm: () => Promise<void>;
  onClose: () => void;
}) {
  const c = machine.confirm;
  const [error, setError] = useState<string | null>(null);
  return (
    <SmallSheet label={c.title} onClose={onClose}>
      <div className="dv-sheet-body" role="alertdialog" aria-label={c.title}>
        <h2 className="dv-sheet-title">{c.title}</h2>
        <p className="dv-sheet-text">{c.message}</p>
        {error && (
          <p className="st-error" role="alert">
            {error}
          </p>
        )}
        <div className="dv-sheet-actions">
          <button type="button" className="dw-btn" onClick={onClose}>
            {c.cancelLabel}
          </button>
          <button
            type="button"
            className="dw-btn dw-btn-primary"
            onClick={() => onConfirm().then(onClose, (e: unknown) => setError(message(e)))}
          >
            {c.confirmLabel}
          </button>
        </div>
      </div>
    </SmallSheet>
  );
}

/** Revoke…: the row's core-owned confirmation. */
export function RevokeSheet({
  row,
  onRevoke,
  onClose,
}: {
  row: DeviceRow;
  onRevoke: () => Promise<void>;
  onClose: () => void;
}) {
  const c = row.revokeConfirm!;
  const [error, setError] = useState<string | null>(null);
  return (
    <SmallSheet label={c.title} onClose={onClose}>
      <div className="dv-sheet-body" role="alertdialog" aria-label={c.title}>
        <h2 className="dv-sheet-title">{c.title}</h2>
        <p className="dv-sheet-text">{c.message}</p>
        {error && (
          <p className="st-error" role="alert">
            {error}
          </p>
        )}
        <div className="dv-sheet-actions">
          <button type="button" className="dw-btn" onClick={onClose}>
            {c.cancelLabel}
          </button>
          <button
            type="button"
            className="dw-btn dw-btn-danger"
            onClick={() => onRevoke().then(onClose, (e: unknown) => setError(message(e)))}
          >
            {c.confirmLabel}
          </button>
        </div>
      </div>
    </SmallSheet>
  );
}
