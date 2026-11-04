// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Launch at login, from the app core (`login_item::*`): the Settings
 * toggle's rows come with the Settings page; this holds the status words,
 * the user's saved choice and what the app does at launch. The SwiftUI app
 * runs the same plan.
 */
import { core } from "../core";
import { readSetting, writeSetting } from "../state/settings";

/** What the system holds for the app as a login item. */
export type LoginItemStatus = "enabled" | "notRegistered" | "requiresApproval" | "notFound";

/** The Settings toggle's state (`SettingsInput.loginItem`). */
export interface LoginItemInput {
  status: LoginItemStatus;
  busy?: boolean;
  error?: string | null;
  /** This machine provides Spaces to your other devices. */
  providesSpaces?: boolean;
  /** This machine runs persistent agents. */
  runsAgents?: boolean;
}

/** What the app does about it at launch. */
export interface LoginItemPlan {
  register: boolean;
  /** Save this as the user's choice. */
  record: boolean | null;
}

/** Where the user's choice is saved (the shell's settings file). */
export const LAUNCH_AT_LOGIN_KEY = "cua.settings.launchAtLogin";

/** The user's choice; null when never made. */
export function readLaunchChoice(): boolean | null {
  const v = readSetting(LAUNCH_AT_LOGIN_KEY, "");
  return v === "true" ? true : v === "false" ? false : null;
}

export function writeLaunchChoice(on: boolean): void {
  writeSetting(LAUNCH_AT_LOGIN_KEY, on ? "true" : "false");
}

/** The core's launch rule (`login_item::launch_plan`). */
export function launchPlan(
  choice: boolean | null,
  onboarded: boolean,
  serves: boolean,
  status: LoginItemStatus,
): LoginItemPlan {
  return core("loginItem.launchPlan", { choice, onboarded, serves, status });
}
