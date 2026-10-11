// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The app core's calls for Settings (About, Devices, Storage, Experiments,
 * launch at login) and Notifications. The feature store and the parity
 * replay both go through these, so what the screens draw is what the
 * parity flows check. Without the core each returns null (or leaves the
 * state as it was): these screens have no TypeScript stand-ins.
 */

import type { CoreClient } from "./core";
import type {
  ApprovalPrompt,
  ApproveSheetAction,
  ApproveSheetState,
  ApproveSheetView,
  DeviceInput,
  DevicesInput,
  DevicesView,
  EnrollAction,
  EnrollState,
  EnrollView,
} from "./contracts/devices";
import type { SettingsInput, SettingsPage, SettingsRow, SettingsSection } from "./contracts/host";
import type { NotificationInput, NotificationsPlan, NotificationsView } from "./contracts/notifications";
import type { AboutInput, AboutView, Experiments, LoginItemInput, StorageAction, StorageInput, StorageRequest, StorageState } from "./contracts/settings";

/* ---- About ----------------------------------------------------------------- */

export const aboutView = (core: CoreClient, input: AboutInput): AboutView | null => core.tryCall<AboutView>("about.view", { input }) ?? null;

/** The Sparkle channels an updater on `channel` installs from. */
export const allowedChannels = (core: CoreClient, channel: string): string[] => core.tryCall<string[]>("about.allowedChannels", { channel }) ?? [];

/* ---- Settings page --------------------------------------------------------- */

/** `settings.page` for a full core input (the bridge's `settingsPage` fills it from the host). */
export const settingsPageOf = (core: CoreClient, input: SettingsInput & Record<string, unknown>): SettingsPage | null =>
  core.tryCall<SettingsPage>("settings.page", { input }) ?? null;

/** Launch at login's rows in General (`login_item::rows_with`): the toggle and its one line. */
export function loginItemRows(core: CoreClient, loginItem: LoginItemInput, experiments: Experiments): SettingsRow[] | null {
  const page = settingsPageOf(core, { loginItem, experiments });
  if (!page) return null;
  return generalLoginRows(page);
}

/** The launch-at-login rows of a Settings page's General section. */
export const generalLoginRows = (page: SettingsPage): SettingsRow[] =>
  page.sections.filter((s) => s.id === "general").flatMap((s) => s.rows.filter((r) => r.id.startsWith("launch-at-login")));

/** The page with Storage after General, only with the Cua Volume experiment on. */
export const withStorage = (core: CoreClient, page: SettingsPage, storage: SettingsSection, experiments: Experiments): SettingsPage | null =>
  core.tryCall<SettingsPage>("settings.withStorage", { page, storage, experiments }) ?? null;

/* ---- Experiments ----------------------------------------------------------- */

export const experimentsPage = (core: CoreClient, experiments: Experiments): SettingsPage | null =>
  core.tryCall<SettingsPage>("experiments.page", { experiments }) ?? null;

/** The switches after a row's choice; unchanged without the core. */
export const chooseExperiment = (core: CoreClient, experiments: Experiments, row: string, option: string): Experiments =>
  core.tryCall<Experiments>("experiments.choose", { experiments, row, option }) ?? experiments;

/* ---- Devices --------------------------------------------------------------- */

/** The Devices page at `now` (Unix seconds). */
export const devicesView = (core: CoreClient, input: DevicesInput, now: number): DevicesView | null => {
  const { readError: _, deviceName: __, ...rest } = input;
  return core.tryCall<DevicesView>("devices.view", { input: rest, now }) ?? null;
};

export const enrollInitial = (core: CoreClient): EnrollState | null => core.tryCall<EnrollState>("devices.enrollInitial", {}) ?? null;
export const enrollReduce = (core: CoreClient, state: EnrollState, action: EnrollAction): EnrollState =>
  core.tryCall<EnrollState>("devices.enrollReduce", { state, action }) ?? state;
export const enrollView = (core: CoreClient, state: EnrollState): EnrollView | null => core.tryCall<EnrollView>("devices.enrollView", { state }) ?? null;

export const approveOpen = (core: CoreClient, prompt: ApprovalPrompt): ApproveSheetState | null =>
  core.tryCall<ApproveSheetState>("devices.approveOpen", { prompt }) ?? null;
export const approveReduce = (core: CoreClient, state: ApproveSheetState, action: ApproveSheetAction, devices: DeviceInput[]): ApproveSheetState =>
  core.tryCall<ApproveSheetState>("devices.approveReduce", { state, action, devices }) ?? state;
export const approveView = (core: CoreClient, state: ApproveSheetState, devices: DeviceInput[]): ApproveSheetView | null =>
  core.tryCall<ApproveSheetView>("devices.approveView", { state, devices }) ?? null;

/** A name as typed for Rename, or null when nothing is left. */
export const cleanDeviceName = (core: CoreClient, name: string): string | null => {
  const r = core.tryCall<string | null>("devices.cleanName", { name });
  return r === undefined ? name.trim() || null : r;
};

/* ---- Storage --------------------------------------------------------------- */

export const storageInitial = (core: CoreClient): StorageState | null => core.tryCall<StorageState>("storage.initial", {}) ?? null;
export const storageReduce = (core: CoreClient, state: StorageState, action: StorageAction): StorageState =>
  core.tryCall<StorageState>("storage.reduce", { state, action }) ?? state;
export const storageSection = (core: CoreClient, input: StorageInput, state: StorageState): SettingsSection | null =>
  core.tryCall<SettingsSection>("storage.section", { input, state }) ?? null;
export const storagePress = (core: CoreClient, input: StorageInput, id: string): StorageAction | null =>
  core.tryCall<StorageAction | null>("storage.press", { input, id }) ?? null;
export const storageChoose = (core: CoreClient, id: string, option: string): StorageAction | null =>
  core.tryCall<StorageAction | null>("storage.choose", { id, option }) ?? null;
export const storageEdit = (core: CoreClient, id: string, value: string): StorageAction | null =>
  core.tryCall<StorageAction | null>("storage.edit", { id, value }) ?? null;
export const storageRequestText = (core: CoreClient, request: StorageRequest): string | null =>
  core.tryCall<string>("storage.requestText", { request }) ?? null;

/* ---- Notifications --------------------------------------------------------- */

/** Which entries to announce, given the last-seen marker (0: first poll, none). */
export const notificationsPlan = (core: CoreClient, feed: NotificationInput[], seenMs: number): NotificationsPlan | null =>
  core.tryCall<NotificationsPlan>("notifications.plan", { feed, seenMs }) ?? null;

export const notificationsView = (core: CoreClient, feed: NotificationInput[], nowMs: number): NotificationsView | null =>
  core.tryCall<NotificationsView>("notifications.view", { feed, nowMs }) ?? null;
