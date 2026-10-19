// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useState } from "react";

import {
  hostFormInitial,
  hostFormView,
  reduceHostForm,
  type HostFormAction,
  type HostFormState,
} from "../model/host";
import type { HostSetupRequest, HostStatus } from "../native/host";

interface HostSetupFormProps {
  /** Runs the host setup (the shell installs the service); awaited so errors show inline. */
  onSetup: (request: HostSetupRequest) => Promise<HostStatus>;
  onBack: () => void;
  onDone: (status: HostStatus) => void;
  /** Signed-in identity; relay mode joins as it. */
  identity?: string;
  /** Show the form's title (the This machine page does; first run has its own). */
  showTitle?: boolean;
}

/**
 * "Set up this machine for unattended access": joins the Cua relay by
 * default, so nothing needs port forwarding; a direct `ip:port` lives under
 * Advanced. The fields, words, checks and the request are the app core's
 * (`host::form_*`); this only draws them and runs the setup.
 */
export function HostSetupForm({ onSetup, onBack, onDone, identity, showTitle = false }: HostSetupFormProps) {
  const [state, setState] = useState<HostFormState>(() => hostFormInitial());
  const view = hostFormView(state, identity);
  const send = (action: HostFormAction) => setState((s) => reduceHostForm(s, action));

  const submit = () => {
    const request = view.request;
    if (!view.canSubmit || !request) return;
    setState((s) => reduceHostForm(s, { type: "submit" }));
    onSetup(request)
      .then((status) => onDone(status))
      .catch((err: unknown) => {
        send({ type: "failed", error: err instanceof Error ? err.message : String(err) });
      });
  };

  const onText = (id: string, value: string) => {
    if (id === "name") send({ type: "set-name", name: value });
    else if (id === "allow") send({ type: "set-allow", allow: value });
    else if (id === "listen") send({ type: "set-listen", listen: value });
    else if (id === "relay") send({ type: "set-relay-url", url: value });
  };

  return (
    <section className="shelf create-cloud host-setup" aria-label={view.title}>
      <div className="create-cloud-body">
        <div className="create-cloud-section">
          {showTitle && <h3 className="host-setup-title">{view.title}</h3>}
          <p className="host-setup-lede">{view.lede}</p>
          {view.fields
            .filter((f) => !f.advanced)
            .map((f) =>
              f.choices && f.choices.length > 0 ? (
                <div className="create-cloud-field" role="radiogroup" aria-label={f.label} key={f.id}>
                  <span>{f.label}</span>
                  <div className="host-profile">
                    {f.choices.map((c) => (
                      <label key={c.id}>
                        <input
                          type="radio"
                          name={`host-${f.id}`}
                          value={c.id}
                          checked={f.value === c.id}
                          onChange={() => send({ type: "set-profile", profile: c.id as "desktop" | "spare" })}
                        />
                        <span>{c.label}</span>
                      </label>
                    ))}
                  </div>
                </div>
              ) : (
              <label className="create-cloud-field" key={f.id}>
                <span>{f.label}</span>
                <input
                  type="text"
                  value={f.value}
                  placeholder={f.placeholder ?? undefined}
                  spellCheck={false}
                  aria-invalid={f.invalid || undefined}
                  onChange={(event) => onText(f.id, event.target.value)}
                />
              </label>
              ),
            )}
          <button
            type="button"
            className="link-button host-advanced-toggle"
            aria-expanded={view.advancedOpen}
            data-owns-enter
            onClick={() => send({ type: "toggle-advanced" })}
          >
            {view.advancedLabel}
          </button>
          {view.advancedOpen && (
            <div className="host-advanced" role="group" aria-label={view.advancedLabel}>
              {view.fields
                .filter((f) => f.advanced)
                .map((f) =>
                  f.toggle ? (
                    <label className="create-cloud-toggle" key={f.id}>
                      <input
                        type="checkbox"
                        checked={f.on}
                        onChange={(event) => send({ type: "set-direct", on: event.target.checked })}
                      />
                      <span>{f.label}</span>
                    </label>
                  ) : (
                    <label className="create-cloud-field" key={f.id}>
                      <span>{f.label}</span>
                      <input
                        type="text"
                        value={f.value}
                        placeholder={f.placeholder ?? undefined}
                        spellCheck={false}
                        aria-invalid={f.invalid || undefined}
                        onChange={(event) => onText(f.id, event.target.value)}
                      />
                    </label>
                  ),
                )}
            </div>
          )}
          {view.error && (
            <p className="create-cloud-error" role="alert">
              {view.error}
            </p>
          )}
        </div>
      </div>
      <footer className="shelf-footer create-cloud-actions">
        <button type="button" className="secondary-button" data-owns-enter onClick={onBack}>
          {view.backLabel}
        </button>
        <button type="button" className="primary-button" data-owns-enter disabled={!view.canSubmit} onClick={submit}>
          {view.submitLabel}
        </button>
      </footer>
    </section>
  );
}
