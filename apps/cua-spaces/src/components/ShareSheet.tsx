// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useState } from 'react';

import {
  reduceShare,
  shareInitial,
  shareErrorWords,
  shareGate,
  shareRemovalNote,
  shareView,
  type ShareEntryInput,
  type ShareInput,
  type ShareRemovalNote,
  type ShareSheetAction,
} from '../model/share';
import type { ShareBridge } from '../native/share';
import { featureSignals, shareSignals } from '../model/telemetry';
import { telemetryBridge } from '../native/telemetry';
import { SmallSheet } from './Devices';
import { useT } from "../i18n";

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * "Share": who can watch or edit the Space, one line each, and an email
 * field. The app core decides every word; this runs its requests.
 */
export function ShareSheet({
  bridge,
  spaceId,
  spaceName,
  signedIn,
  shareable,
  onClose,
  onOpenExternal,
}: {
  bridge: ShareBridge;
  spaceId: string;
  spaceName: string;
  signedIn: boolean;
  shareable: boolean;
  onClose: () => void;
  /** Opens a link (See Teams) in the browser. */
  onOpenExternal?: (url: string) => void;
}) {
  const t = useT("sharing");
  const [shares, setShares] = useState<ShareEntryInput[]>([]);
  // In a team that can share; null until read, or when it cannot be said.
  const [hasTeam, setHasTeam] = useState<boolean | null>(null);
  // Sharing adoption: the sheet was opened.
  useEffect(() => telemetryBridge().recordSignals(featureSignals('share_open')), []);
  const [state, setState] = useState(shareInitial);
  const input: ShareInput = { spaceId, spaceName, shares, signedIn, shareable };
  const v = shareView(input, state);
  const gate = signedIn && shareable ? shareGate(hasTeam) : null;
  const closed = Boolean(v.disabledReason) || gate !== null;

  useEffect(() => {
    if (!signedIn) return;
    void bridge.shares(spaceId).then(
      (r) => setShares(r.shares),
      () => {}
    );
  }, [bridge, spaceId, signedIn]);

  useEffect(() => {
    if (!signedIn) return;
    void bridge.hasTeam().then(setHasTeam, () => {});
  }, [bridge, signedIn]);

  const run = useCallback(
    async (action: ShareSheetAction) => {
      const next = reduceShare(input, state, action);
      setState(next);
      const request = next.request;
      const starts =
        action.type === 'submit' || action.type === 'change-role' || action.type === 'remove';
      if (!starts || !request || state.busy) return;
      try {
        const r =
          request.kind === 'share'
            ? await bridge.share(request.space, request.who, request.role)
            : await bridge.unshare(request.space, request.who);
        setShares(r.shares);
        // Shared (view-only or not), unshared or a role changed: never who.
        telemetryBridge().recordSignals(shareSignals(input, next, { type: 'done' }));
        setState((s) => reduceShare({ ...input, shares: r.shares }, s, { type: 'done' }));
      } catch (e) {
        // A refusal for who is not in the team (or no team) reads in plain words.
        const raw = message(e);
        const failed: ShareSheetAction = {
          type: 'failed',
          error: shareErrorWords(raw, request.kind === 'share' ? request.who : '') ?? raw,
        };
        telemetryBridge().recordSignals(shareSignals(input, next, failed));
        setState((s) => reduceShare(input, s, failed));
      }
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [bridge, state, shares, signedIn, shareable, spaceId, spaceName]
  );

  const roleSelect = (value: string, onChange: (role: string) => void, label: string) => (
    <select
      className="dw-input"
      aria-label={label}
      value={value}
      disabled={v.busy || closed}
      onChange={(e) => onChange(e.target.value)}
    >
      {v.roles.map((r) => (
        <option key={r.id} value={r.id}>
          {r.label}
        </option>
      ))}
    </select>
  );

  return (
    <SmallSheet label={v.title} onClose={onClose}>
      <form
        className="dv-sheet-body"
        onSubmit={(event) => {
          event.preventDefault();
          void run({ type: 'submit' });
        }}
      >
        <h2 className="dv-sheet-title">{v.title}</h2>
        {(v.disabledReason || gate) && (
          <p className="dv-sheet-text">{v.disabledReason ?? gate?.reason}</p>
        )}
        {!v.disabledReason && gate && (
          <button
            type="button"
            className="dw-btn"
            onClick={() => onOpenExternal?.(gate.action.url)}
          >
            {gate.action.label}
          </button>
        )}
        <div className="sh-add">
          <input
            className="dw-input"
            placeholder={v.whoPlaceholder}
            aria-label={v.whoPlaceholder}
            value={v.who}
            autoFocus
            spellCheck={false}
            autoComplete="off"
            disabled={v.busy || closed}
            onChange={(event) => void run({ type: 'set-who', who: event.target.value })}
          />
          {roleSelect(v.role, (role) => void run({ type: 'set-role', role }), t("sharing.sheet.role_label"))}
          <button type="submit" className="dw-btn dw-btn-primary" disabled={!v.canShare || closed}>
            {v.shareLabel}
          </button>
        </div>
        {v.hint && <p className="dv-sheet-text">{v.hint}</p>}
        <ul className="sh-rows" aria-label={t("sharing.sheet.shared_with_label")}>
          {v.rows.length === 0 && <li className="sh-empty">{v.emptyText}</li>}
          {v.rows.map((r) => (
            <li key={r.who} className="sh-row">
              <span
                className={`status-dot ${r.connected ? 'status-running' : 'status-suspended'}`}
                aria-hidden="true"
              />
              <span className="sh-who">{r.who}</span>
              {roleSelect(
                r.role,
                (role) => void run({ type: 'change-role', who: r.who, role }),
                `Role of ${r.who}`
              )}
              <button
                type="button"
                className="dw-btn"
                disabled={v.busy}
                onClick={() => void run({ type: 'remove', who: r.who })}
              >
                {v.removeLabel}
              </button>
            </li>
          ))}
        </ul>
        {v.error && (
          <p className="st-error" role="alert">
            {v.error}
          </p>
        )}
        <div className="dv-sheet-actions">
          <button type="button" className="dw-btn" onClick={onClose}>
            {v.doneLabel}
          </button>
        </div>
      </form>
    </SmallSheet>
  );
}

/**
 * Sharing is for teams (CUA-1883): the owner sees once, in one line, who can
 * no longer open a shared computer, with "Invite to your team". The words are
 * the app core's (`share.removalNote`); closing it tells the account it was
 * seen, so it does not come back.
 */
export function ShareRemovalBanner({
  bridge,
  signedIn,
  onOpenExternal,
}: {
  bridge: ShareBridge;
  signedIn: boolean;
  onOpenExternal: (url: string) => void;
}) {
  const [note, setNote] = useState<ShareRemovalNote | null>(null);
  useEffect(() => {
    if (!signedIn) return;
    let live = true;
    void bridge.removals().then(
      (removals) => live && setNote(shareRemovalNote(removals)),
      () => {}
    );
    return () => {
      live = false;
    };
  }, [bridge, signedIn]);
  if (!note) return null;
  const dismiss = () => {
    setNote(null);
    void bridge.ackRemovals().catch(() => {});
  };
  return (
    <div className="dw-banner" data-tone="info" role="status" data-share-removal="">
      <span>{note.text}</span>
      <button type="button" className="dw-btn" onClick={() => onOpenExternal(note.action.url)}>
        {note.action.label}
      </button>
      <button type="button" className="dw-btn" onClick={dismiss}>
        {note.dismiss}
      </button>
    </div>
  );
}
