// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The first run, as the app core lays it out.
 *
 * Hand-written mirror of `libs/cua/crates/cua-spaces-app-core/src/
 * onboarding.rs` (`OnboardingState`, `OnboardingAction`, `OnboardingView`,
 * `OnboardingCopy`), `host.rs` (`HostPermissionInput`, `PermissionRow`) and
 * `wizard.rs` (`SandboxImage`, trimmed). Rust is the source of truth; these
 * are its JSON (camelCase) shapes. The SwiftUI app draws the same views
 * (`OnboardingView.swift`).
 */

import type { OnboardingMode } from "./host";
import type { DriveCard, DriveCheckInput, DriveMountInput, DriveStorageInput, Experiments, StorageAction, StorageChoice } from "./volume";
import type { SpaceKind, SpaceOs } from "./spaces";

/** A page (`OnboardingStep`), in the core's order. */
export type OnboardingStep = "welcome" | "signin" | "agents" | "presentation" | "drive" | "mode" | "done";

/** `TelemetryInput`: the machine's setting, and what decides it when not the user. */
export interface OnboardingTelemetry {
  enabled: boolean;
  lockedBy?: string | null;
  /** The usage notice was already shown on this machine (Welcome then
   * counts the run as it shows, not when it is left). */
  noticeShown?: boolean;
}

/**
 * `OnboardingState`. The UI never edits it: it sends actions through the
 * core's reducer and keeps the result (saved, so the flow resumes). The
 * fields the web pages read are typed; the rest pass through untouched.
 */
export interface OnboardingFlowState {
  step: OnboardingStep;
  identity?: string | null;
  mode?: OnboardingMode | null;
  installerMode?: OnboardingMode | null;
  menuBar: boolean;
  telemetry?: OnboardingTelemetry | null;
  /** The run's start (`onboarding_shown`) was counted. */
  runCounted?: boolean;
  [field: string]: unknown;
}

/** The `OnboardingAction`s a web host can send (kebab-case `type`). */
export type OnboardingAction =
  | { type: "start" }
  | { type: "signed-in"; identity: string }
  | { type: "signin-done" }
  | { type: "agents-done"; configured: string[] }
  | { type: "presentation-picked"; menuBar: boolean }
  | { type: "presentation-done" }
  | { type: "mode-chosen"; mode: OnboardingMode }
  | { type: "back" }
  | { type: "telemetry-loaded"; telemetry: OnboardingTelemetry }
  | { type: "usage-data-toggled"; on: boolean }
  /** Done's "Launch at login" checkbox. */
  | { type: "launch-at-login-toggled"; on: boolean }
  /** Welcome is on screen (after `telemetry-loaded`). */
  | { type: "welcome-shown" }
  // The Cua Volume page (`volume.ts` sends these for the host's answers).
  | { type: "experiments-loaded"; experiments: Experiments }
  | { type: "drive-checked"; os: SpaceOs; status: DriveMountInput | null }
  | { type: "drive-toggled"; on: boolean }
  | { type: "drive-continue" }
  | { type: "drive-mounted"; status: DriveMountInput }
  | { type: "drive-failed"; error: string }
  | { type: "drive-storage-loaded"; storage: DriveStorageInput; home: string | null }
  | { type: "storage-chosen"; choice: StorageChoice }
  | { type: "drive-storage"; action: StorageAction }
  | { type: "drive-storage-saved"; check: DriveCheckInput };

export interface OnboardingDot {
  step: OnboardingStep;
  label: string;
  current: boolean;
}

/** `spaces::sidebar::Fact`, as Done's summary uses it. */
export interface OnboardingFact {
  label: string;
  value: string;
}

export interface OnboardingModeChoice {
  mode: OnboardingMode;
  label: string;
  preselected: boolean;
}

/** Welcome's "Share anonymous usage data" switch. */
export interface OnboardingUsage {
  label: string;
  on: boolean;
  /** False while the environment decides (`DO_NOT_TRACK`, CI). */
  enabled: boolean;
  /** Why it cannot change ("Set by env DO_NOT_TRACK"). */
  help?: string | null;
}

/** `OnboardingView`: the page as drawn. */
export interface OnboardingView {
  step: OnboardingStep;
  dots: OnboardingDot[];
  title: string;
  lede: string;
  primaryLabel: string;
  canSkip: boolean;
  canBack: boolean;
  showMark: boolean;
  preselectedMode: OnboardingMode;
  summary: OnboardingFact[];
  choices: OnboardingModeChoice[];
  notice?: string | null;
  noticeLinkLabel?: string | null;
  noticeLinkUrl?: string | null;
  usage?: OnboardingUsage | null;
  /** The Cua Volume page's card (on its page only). */
  drive?: DriveCard | null;
  /** Done's "Launch at login" checkbox (on Done only). */
  launchAtLogin?: OnboardingCheckbox | null;
}

/** `OnboardingCheckbox`: Done's "Launch at login". */
export interface OnboardingCheckbox {
  label: string;
  checked: boolean;
  /** The line under it. */
  note: string;
}

/** `OnboardingCopy`: the fixed words inside the pages (the fields the web uses). */
export interface OnboardingCopy {
  back: string;
  skip: string;
  continueLabel: string;
  tryAgain: string;
  signIn: string;
  signInWaiting: string;
  permissionsTitle: string;
  openSettings: string;
  teams: string;
  teamsLink: string;
  teamsUrl: string;
}

/** `host::PermissionRow`: a macOS pane still to grant. */
export interface PermissionRow {
  id: string;
  /** "Screen Recording". */
  title: string;
  /** What to turn on there. */
  help: string;
  /** The pane (`x-apple.systempreferences:`), when known. */
  settingsUrl?: string | null;
}

/** `wizard::SandboxImage`, trimmed to what the first-Space picker reads. */
export interface SandboxImage {
  ref: string;
  group: string;
  os: SpaceOs;
  name: string;
  variant: SpaceKind;
  summary: string;
  tier?: "slim" | "full" | "xcode" | null;
  published: boolean;
}
