// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useState } from 'react';

import {
  cloudConnectInitial,
  cloudConnectView,
  reduceCloudConnect,
  type CloudConnectAction,
  type CloudConnectInput,
  type CloudConnectState,
} from '../model/cloudConnect';
import { connectInput, type CloudBridge } from '../native/cloud';
import { SmallSheet } from './Devices';

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * "Connect a cloud": one line per provider (a check mark when this machine
 * has its sign-in) plus "No cloud", the region, project or environment,
 * what the selected provider touches, Test and Connect. The app core
 * decides every word; this runs its requests.
 */
export function ConnectCloudSheet({
  bridge,
  onConnected,
  onClose,
}: {
  bridge: CloudBridge;
  /** Connected: the sheet closes and the wizard refreshes its clouds. */
  onConnected: () => void;
  onClose: () => void;
}) {
  const [input, setInput] = useState<CloudConnectInput>({ providers: [] });
  const [state, setState] = useState<CloudConnectState>(cloudConnectInitial);
  const v = cloudConnectView(input, state);

  useEffect(() => {
    void bridge.status().then(
      (s) => setInput(connectInput(s)),
      () => {}
    );
  }, [bridge]);

  const run = useCallback(
    async (action: CloudConnectAction) => {
      const next = reduceCloudConnect(input, state, action);
      setState(next);
      if (action.type !== 'test' && action.type !== 'connect') return;
      const nextView = cloudConnectView(input, next);
      // "No cloud" (and any other no-request Connect) finishes the flow on
      // the spot, with no cloud_test / cloud_connect call at all.
      if (nextView.done) {
        onClose();
        return;
      }
      const request = nextView.request;
      if (!request) return;
      try {
        if (request.kind === 'test') {
          const r = await bridge.test(request.target);
          setState((s) =>
            reduceCloudConnect(input, s, {
              type: 'tested',
              ok: r.ok,
              account: r.account ?? '',
              checks: r.checks.map((c) => ({ name: c.name, ok: c.ok, detail: c.detail ?? '' })),
            })
          );
        } else {
          const r = await bridge.connect(request.target, request.make_default);
          setState((s) => reduceCloudConnect(input, s, { type: 'connected', label: r.label ?? r.title }));
          onConnected();
        }
      } catch (e) {
        setState((s) => reduceCloudConnect(input, s, { type: 'failed', error: message(e) }));
      }
    },
    [bridge, input, state, onConnected, onClose]
  );

  const text = (f: NonNullable<typeof v.field>, type: 'set-value' | 'set-profile') => (
    <label className="dw-field">
      <span>{f.label}</span>
      <input
        className="dw-input"
        aria-label={f.label}
        placeholder={f.placeholder}
        value={f.value}
        spellCheck={false}
        autoComplete="off"
        onChange={(event) => void run({ type, text: event.target.value })}
      />
    </label>
  );

  return (
    <SmallSheet label={v.title} onClose={onClose}>
      <form
        className="dv-sheet-body"
        onSubmit={(event) => {
          event.preventDefault();
          void run({ type: 'connect' });
        }}
      >
        <h2 className="dv-sheet-title">{v.title}</h2>
        <ul className="sh-rows" role="radiogroup" aria-label="Clouds">
          {v.rows.map((r) => (
            <li key={r.id} className="sh-row">
              <label className="dw-check">
                <input
                  type="radio"
                  name="cloud"
                  checked={r.selected}
                  onChange={() => void run({ type: 'select', name: r.id })}
                />
                <span className="sh-who">{r.title}</span>
                <span className="dv-sheet-text">{r.detail}</span>
                {r.found && <span aria-label="Sign-in found" title="Sign-in found">{'✓'}</span>}
              </label>
            </li>
          ))}
        </ul>
        {v.field && text(v.field, 'set-value')}
        {v.profileField && text(v.profileField, 'set-profile')}
        {v.field && (
          <label className="dw-check">
            <input
              type="checkbox"
              checked={v.makeDefault}
              onChange={(event) => void run({ type: 'set-make-default', on: event.target.checked })}
            />
            <span>{v.makeDefaultLabel}</span>
          </label>
        )}
        {v.touches.length > 0 && (
          <div aria-label="What Cua will touch">
            {v.touches.map((line) => (
              <p key={line} className="dv-sheet-text">
                {line}
              </p>
            ))}
          </div>
        )}
        {v.checks.length > 0 && (
          <ul className="sh-rows" aria-label="Checks">
            {v.checks.map((c) => (
              <li key={c.text} className="sh-row">
                <span className={`status-dot ${c.ok ? 'status-running' : 'status-error'}`} aria-hidden="true" />
                <span>{c.text}</span>
              </li>
            ))}
          </ul>
        )}
        {v.result && <p className="dv-sheet-text">{v.result}</p>}
        {v.error && (
          <p className="st-error" role="alert">
            {v.error}
          </p>
        )}
        <div className="dv-sheet-actions">
          <button type="button" className="dw-btn" onClick={onClose}>
            {v.cancelLabel}
          </button>
          <button
            type="button"
            className="dw-btn"
            title={v.testHelp}
            disabled={!v.canTest}
            onClick={() => void run({ type: 'test' })}
          >
            {v.testLabel}
          </button>
          <button type="submit" className="dw-btn dw-btn-primary" disabled={!v.canConnect}>
            {v.connectLabel}
          </button>
        </div>
      </form>
    </SmallSheet>
  );
}
