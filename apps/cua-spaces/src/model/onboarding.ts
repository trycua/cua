// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * First run, from the app core (`onboarding::*`): the page order, each
 * page's title, line and buttons, the This machine answers, the Done facts
 * and the fixed words inside the steps. The SwiftUI app renders the same.
 */
import { core } from "../core";
import type { DriveCheckInput, DriveStorageInput, StorageAction, StorageState } from "./driveSettings";
import type { DriveMountInput } from "./persistent";
import type { SpaceOs } from "./types";
import type { SettingsRow } from "./window";

/** Where the volume's files live, as picked on its page. */
export type StorageChoice = "local" | "s3" | "later";

export type OnboardingStep = "welcome" | "signin" | "agents" | "presentation" | "drive" | "mode" | "done";
export type OnboardingModeWord = "client" | "host";

export interface OnboardingState {
  step: OnboardingStep;
  cliTarget: string | null;
  /** Menu bar only (the "Spaces tab in the notch" setting). */
  menuBar: boolean;
  identity: string | null;
  agents: string[];
  mode: OnboardingModeWord | null;
  installerMode: OnboardingModeWord | null;
  /** This machine's system, once the drive was checked. */
  driveOs?: SpaceOs | null;
  /** `volume_mount_status`, when the daemon answered. */
  driveStatus?: DriveMountInput | null;
  /** The drive was checked (answered or not). */
  driveChecked?: boolean;
  /** The Cua Volume checkbox (off by default). */
  driveMount?: boolean;
  /** What the shell runs for the Cua Volume page: `volume_mount` or `volume_unmount`. */
  driveRequest?: "mount" | "unmount" | null;
  driveError?: string | null;
  /** `volume_storage`, when the daemon answered. */
  driveStorage?: DriveStorageInput | null;
  /** The home folder (paths show as `~/...`). */
  driveHome?: string | null;
  storageChoice?: StorageChoice;
  /** The user picked a storage option (the saved backend no longer leads). */
  storagePicked?: boolean;
  /** The bucket form and its test or save (the Storage section's state). */
  storage?: StorageState;
  /** Done's "Launch at login" checkbox (ticked unless unticked). */
  launchAtLogin?: boolean;
  /** The machine's telemetry setting, as read (Welcome's usage-data switch). */
  telemetry?: { enabled: boolean; lockedBy: string | null } | null;
  /** Settings, Experiments: the Cua Volume page only with Cua Volume on. */
  experiments?: import("./experiments").Experiments;
}

export type OnboardingAction =
  | { type: "start" }
  | { type: "cli-installed"; target: string | null }
  | { type: "presentation-picked"; menuBar: boolean }
  | { type: "presentation-done" }
  | { type: "signed-in"; identity: string }
  | { type: "signin-done" }
  | { type: "agents-done"; configured: string[] }
  | { type: "mode-chosen"; mode: OnboardingModeWord }
  | { type: "telemetry-loaded"; telemetry: { enabled: boolean; lockedBy: string | null } }
  | { type: "usage-data-toggled"; on: boolean }
  | { type: "drive-checked"; os: SpaceOs; status: DriveMountInput | null }
  | { type: "drive-toggled"; on: boolean }
  | { type: "drive-continue" }
  | { type: "drive-mounted"; status: DriveMountInput }
  | { type: "drive-failed"; error: string }
  | { type: "drive-storage-loaded"; storage: DriveStorageInput; home: string | null }
  | { type: "storage-chosen"; choice: StorageChoice }
  | { type: "drive-storage"; action: StorageAction }
  | { type: "drive-storage-saved"; check: DriveCheckInput }
  | { type: "launch-at-login-toggled"; on: boolean }
  | { type: "experiments-loaded"; experiments: import("./experiments").Experiments }
  | { type: "back" };

export interface OnboardingView {
  step: OnboardingStep;
  dots: { step: OnboardingStep; label: string; current: boolean }[];
  title: string;
  lede: string;
  primaryLabel: string;
  canSkip: boolean;
  canBack: boolean;
  showMark: boolean;
  preselectedMode: OnboardingModeWord;
  summary: { label: string; value: string }[];
  choices: { mode: OnboardingModeWord; label: string; preselected: boolean }[];
  /** The presentation page's two cards (an animated miniature each). */
  presentations: PresentationCard[];
  /** Done's example prompts, scrolling slowly under both columns. */
  prompts: string[];
  notice: string | null;
  noticeLinkLabel: string | null;
  noticeLinkUrl: string | null;
  /** Welcome's "Share anonymous usage data" switch (Welcome only). */
  usage: { label: string; on: boolean; enabled: boolean; help: string | null } | null;
  /** The Cua Volume page's card. */
  drive: DriveCard | null;
  /** Done's "Launch at login" checkbox. */
  launchAtLogin: { label: string; checked: boolean; note: string } | null;
}

/** The Cua Volume page's card: the miniature over one checkbox line. */
export interface DriveCard {
  label: string;
  imageLabel: string;
  checked: boolean;
  enabled: boolean;
  busy: boolean;
  note: string | null;
  error: string | null;
  settingsLabel: string | null;
  settingsUrl: string | null;
  /** "Where your files live" (null until `volume_storage` answered). */
  storageTitle: string | null;
  /** This Mac, Your S3 bucket, Set up later. */
  storageOptions: { id: StorageChoice; label: string; active: boolean }[];
  /** The bucket's rows (Settings rows) when a bucket is picked. */
  storageRows: SettingsRow[];
  /** One muted line (Set up later). */
  storageNote: string | null;
  canContinue: boolean;
  /** "Stored in ~/.cua/volume/data", and the full path. */
  storedIn: string | null;
  storedPath: string | null;
  /** "In Finder at ~/Cua Volume" while mounted, and the full path. */
  mountedAt: string | null;
  mountedPath: string | null;
}

export interface PresentationCard {
  menuBar: boolean;
  title: string;
  /** `onboarding-notch` or `onboarding-menu-bar`: the list key. */
  id: string;
  imageLabel: string;
  selected: boolean;
}

export interface OnboardingCopy {
  back: string;
  skip: string;
  continueLabel: string;
  tryAgain: string;
  checking: string;
  installScript: string;
  install: string;
  installing: string;
  addToPath: string;
  shadowed: string;
  signIn: string;
  signInWaiting: string;
  agentsLooking: string;
  agentsNone: string;
  agentsSkills: string;
  agentsMcp: string;
  /** The background computer-use card's one line. */
  agentsDriver: string;
  /** The card miniature's accessibility label. */
  agentsDriverImage: string;
  agentsSetUp: string;
  agentsSettingUp: string;
  agentsDoneTitle: string;
  agentsDoneLede: string;
  permissionsTitle: string;
  openSettings: string;
  /** The Sign in page's Teams line ("Teams · coming soon"). */
  teams: string;
  /** Its link ("Join the waitlist"). */
  teamsLink: string;
  /** The website's Teams waitlist (the app collects nothing). */
  teamsUrl: string;
}

export function initialOnboarding(installerMode?: OnboardingModeWord | null, identity?: string | null): OnboardingState {
  return core("onboarding.initial", { installerMode: installerMode ?? null, identity: identity ?? null });
}

export function reduceOnboarding(state: OnboardingState, action: OnboardingAction): OnboardingState {
  return core("onboarding.reduce", { state, action });
}

export function onboardingView(state: OnboardingState): OnboardingView {
  return core("onboarding.view", { state });
}

export function onboardingCopy(): OnboardingCopy {
  return core("onboarding.copy");
}

export const signedInText = (identity?: string | null): string =>
  core("onboarding.signedInText", { identity: identity ?? null });
export const signInCodeText = (userCode?: string | null): string =>
  core("onboarding.signInCodeText", { userCode: userCode ?? null });
export const replacesText = (installedVersion?: string | null): string =>
  core("onboarding.replacesText", { installedVersion: installedVersion ?? null });
export const installedAtText = (target: string): string => core("onboarding.installedAtText", { target });
