// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { type ReactNode, useEffect, useState } from "react";

import { hostFormInitial, hostFormView } from "../model/host";
import { initialOnboarding, onboardingView, type OnboardingView } from "../model/onboarding";
import type { HostBridge, HostStatus, OnboardingMode } from "../native/host";
import { HostSetupForm } from "./HostSetupForm";
import { Page } from "./desktop/OnboardingPage";

/**
 * First run: what is this machine for?
 *
 * 1. Access other machines: nothing is installed; "This machine" stays in the
 *    roster with "Set up for access" for later.
 * 2. Set up this machine for unattended access: installs cua-spacesd as a
 *    service that joins the Cua relay (or, under Advanced, listens on a
 *    direct ip:port).
 *
 * The answers, the notice and the form are the app core's; permission panes
 * left to grant show on Done. An installer can preselect the choice
 * (`--mode host|client`, or the MDM install-mode file); the user still
 * confirms here.
 */
export function Onboarding({
  host,
  view: viewProp,
  back,
  installerMode,
  identity,
  onDone,
}: {
  host: HostBridge;
  /** The This machine page (the core's onboarding view at `mode`). */
  view?: OnboardingView;
  back?: ReactNode;
  installerMode?: OnboardingMode | null;
  identity?: string;
  onDone: (mode: OnboardingMode, status?: HostStatus) => void;
}) {
  const view = viewProp ?? onboardingView({ ...initialOnboarding(installerMode ?? null, identity ?? null), step: "mode" });
  const [form, setForm] = useState(view.preselectedMode === "host");
  // Choosing only selects; Continue (or Enter) moves on. "Access other
  // machines" is the default so the page always has an enabled way forward.
  const [selected, setSelected] = useState<OnboardingMode>(view.preselectedMode ?? "client");

  // The first run's telemetry (its notice on the Welcome page, the pages
  // and Done) is the installer flow's, from the app core.
  const finish = (mode: OnboardingMode, status?: HostStatus) => {
    void host
      .completeOnboarding(mode)
      .catch(() => {})
      .finally(() => onDone(mode, status));
  };

  const advance = () => (selected === "host" ? setForm(true) : finish("client"));

  // Enter continues unless a button or field has focus and handles it itself.
  useEffect(() => {
    if (form) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Enter" || e.defaultPrevented) return;
      const t = e.target as HTMLElement | null;
      if (t && t.closest("button, a, input, textarea, select")) return;
      e.preventDefault();
      advance();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  if (form) {
    const formTitle = hostFormView(hostFormInitial(), identity).title;
    return (
      <Page title={formTitle} lede={null}>
        <HostSetupForm
          identity={identity}
          onSetup={(request) => host.setup(request)}
          onBack={() => setForm(false)}
          onDone={(status) => finish("host", status)}
        />
      </Page>
    );
  }

  return (
    <Page
      title={view.title}
      lede={view.lede}
      back={back ?? undefined}
      primary={
        <button type="button" className="primary-button" data-owns-enter autoFocus onClick={advance}>
          {view.primaryLabel}
        </button>
      }
    >
      <div className="new-space-cards" role="radiogroup" aria-label={view.title}>
        {view.choices.map((c) => (
          <button
            key={c.mode}
            type="button"
            role="radio"
            aria-checked={c.mode === selected}
            className={`new-space-card${c.mode === selected ? " new-space-card-primary" : ""}`}
            data-owns-enter
            onClick={() => setSelected(c.mode)}
          >
            <span className="new-space-card-title">{c.label}</span>
          </button>
        ))}
      </div>
      <p className="host-setup-lede" data-testid="mode-later">
        Change it later from This machine in your Spaces list.
      </p>
      {view.notice && (
        <p className="onboarding-telemetry-notice" data-testid="telemetry-notice">
          {view.notice}{" "}
          {view.noticeLinkUrl && (
            <a href={view.noticeLinkUrl} target="_blank" rel="noreferrer">
              {view.noticeLinkLabel}
            </a>
          )}
        </p>
      )}
    </Page>
  );
}
