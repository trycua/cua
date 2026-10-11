// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { type ReactNode, useState } from "react";

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

  // The first run's telemetry (its notice on the Welcome page, the pages
  // and Done) is the installer flow's, from the app core.
  const finish = (mode: OnboardingMode, status?: HostStatus) => {
    void host
      .completeOnboarding(mode)
      .catch(() => {})
      .finally(() => onDone(mode, status));
  };

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
    <Page title={view.title} lede={view.lede} back={back ?? undefined}>
      <div className="new-space-cards" role="group" aria-label={view.title}>
        {view.choices.map((c) => (
          <button
            key={c.mode}
            type="button"
            className={`new-space-card${c.preselected ? " new-space-card-primary" : ""}`}
            data-owns-enter
            onClick={() => (c.mode === "host" ? setForm(true) : finish("client"))}
          >
            <span className="new-space-card-title">{c.label}</span>
          </button>
        ))}
      </div>
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
