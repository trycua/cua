// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useState } from 'react';

import {
  reduceShare,
  shareInitial,
  shareView,
  type ShareEntryInput,
  type ShareInput,
  type ShareSheetAction,
} from '../model/share';
import type { ShareBridge } from '../native/share';
import { featureSignals, shareSignals } from '../model/telemetry';
import { telemetryBridge } from '../native/telemetry';
import { SmallSheet } from './Devices';

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
}: {
  bridge: ShareBridge;
  spaceId: string;
  spaceName: string;
  signedIn: boolean;
  shareable: boolean;
  onClose: () => void;
}) {
  const [shares, setShares] = useState<ShareEntryInput[]>([]);
  // Sharing adoption: the sheet was opened.
  useEffect(() => telemetryBridge().recordSignals(featureSignals('share_open')), []);
  const [state, setState] = useState(shareInitial);
  const input: ShareInput = { spaceId, spaceName, shares, signedIn, shareable };
  const v = shareView(input, state);

  useEffect(() => {
    if (!signedIn) return;
    void bridge.shares(spaceId).then(
      (r) => setShares(r.shares),
      () => {}
    );
  }, [bridge, spaceId, signedIn]);

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
        const failed: ShareSheetAction = { type: 'failed', error: message(e) };
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
      disabled={v.busy || Boolean(v.disabledReason)}
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
        {v.disabledReason && <p className="dv-sheet-text">{v.disabledReason}</p>}
        <div className="sh-add">
          <input
            className="dw-input"
            placeholder={v.whoPlaceholder}
            aria-label={v.whoPlaceholder}
            value={v.who}
            autoFocus
            spellCheck={false}
            autoComplete="off"
            disabled={v.busy || Boolean(v.disabledReason)}
            onChange={(event) => void run({ type: 'set-who', who: event.target.value })}
          />
          {roleSelect(v.role, (role) => void run({ type: 'set-role', role }), 'Role')}
          <button type="submit" className="dw-btn dw-btn-primary" disabled={!v.canShare}>
            {v.shareLabel}
          </button>
        </div>
        {v.hint && <p className="dv-sheet-text">{v.hint}</p>}
        <ul className="sh-rows" aria-label="Shared with">
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
