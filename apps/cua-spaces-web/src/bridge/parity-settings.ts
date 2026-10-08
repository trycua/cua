// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The parity replay for Settings (About, Devices, Storage, Experiments,
 * launch at login) and Notifications: the core calls these screens make go
 * through the bridge's own functions (`settings-derive.ts`), and the states
 * they reach are recorded for the Playwright harness to put on the real
 * screens (`window.__cuaParity.settings`). See `parity.ts`.
 */

import type { CoreClient } from "./core";
import type {
  ApproveSheetState,
  ApproveSheetView,
  DeviceInput,
  DevicesInput,
  DevicesView,
  EnrollState,
  EnrollView,
} from "./contracts/devices";
import type { SettingsPage, SettingsRow, SettingsSection } from "./contracts/host";
import type { NotificationInput, NotificationsView } from "./contracts/notifications";
import type { AboutInput, AboutView, Experiments, LoginItemInput, StorageInput, StorageState } from "./contracts/settings";
import {
  aboutView,
  allowedChannels,
  approveOpen,
  approveReduce,
  approveView,
  chooseExperiment,
  devicesView,
  enrollInitial,
  enrollReduce,
  enrollView,
  experimentsPage,
  generalLoginRows,
  notificationsPlan,
  notificationsView,
  settingsPageOf,
  storageChoose,
  storageEdit,
  storageInitial,
  storagePress,
  storageReduce,
  storageRequestText,
  storageSection,
  withStorage,
} from "./settings-derive";
import type { BridgeStore } from "./store";

type Args = Record<string, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

/** A Settings or Notifications state, as the replay reached it. */
export type SettingsCheckpoint =
  | { kind: "about"; input: AboutInput; view: AboutView }
  | { kind: "login-item"; loginItem: LoginItemInput; experiments: Experiments; rows: SettingsRow[] }
  | { kind: "experiments-tab"; experiments: Experiments; page: SettingsPage }
  | { kind: "with-storage"; experiments: Experiments; sections: string[] }
  | { kind: "devices"; input: DevicesInput; now: number; view: DevicesView }
  | { kind: "enroll"; state: EnrollState; view: EnrollView }
  | { kind: "approve"; state: ApproveSheetState; devices: DeviceInput[]; view: ApproveSheetView }
  | { kind: "storage"; input: StorageInput; state: StorageState; section: SettingsSection }
  | { kind: "notifications"; feed: NotificationInput[]; nowMs: number; view: NotificationsView };

/** The bridge's answer to each Settings and Notifications core call. */
export function settingsParityMethods(core: CoreClient, record: (c: SettingsCheckpoint) => void): Record<string, (a: Args) => unknown> {
  const seen = <T>(v: T | null, f: (v: T) => void): T | null => {
    if (v !== null) f(v);
    return v;
  };
  return {
    "about.view": (a) => seen(aboutView(core, a.input), (view) => record({ kind: "about", input: a.input, view })),
    "about.allowedChannels": (a) => allowedChannels(core, a.channel),

    "settings.page": (a) =>
      seen(settingsPageOf(core, a.input), (page) => {
        if (a.input.loginItem) {
          const loginItem: LoginItemInput = { status: "notRegistered", busy: false, error: null, providesSpaces: false, runsAgents: false, ...a.input.loginItem };
          record({ kind: "login-item", loginItem, experiments: a.input.experiments ?? {}, rows: generalLoginRows(page) });
        }
      }),
    "settings.withStorage": (a) =>
      seen(withStorage(core, a.page, a.storage, a.experiments), (page) =>
        record({ kind: "with-storage", experiments: a.experiments, sections: page.sections.map((s) => s.id) }),
      ),
    "experiments.page": (a) => seen(experimentsPage(core, a.experiments), (page) => record({ kind: "experiments-tab", experiments: a.experiments, page })),
    "experiments.choose": (a) => chooseExperiment(core, a.experiments, a.row, a.option),

    "devices.view": (a) => seen(devicesView(core, a.input, a.now), (view) => record({ kind: "devices", input: a.input, now: a.now, view })),
    "devices.enrollInitial": () => enrollInitial(core),
    "devices.enrollReduce": (a) => enrollReduce(core, a.state, a.action),
    "devices.enrollView": (a) => seen(enrollView(core, a.state), (view) => record({ kind: "enroll", state: a.state, view })),
    "devices.approveOpen": (a) => approveOpen(core, a.prompt),
    "devices.approveReduce": (a) => approveReduce(core, a.state, a.action, a.devices),
    "devices.approveView": (a) =>
      seen(approveView(core, a.state, a.devices), (view) => record({ kind: "approve", state: a.state, devices: a.devices, view })),

    "storage.initial": () => storageInitial(core),
    "storage.reduce": (a) => storageReduce(core, a.state, a.action),
    "storage.section": (a) => seen(storageSection(core, a.input, a.state), (section) => record({ kind: "storage", input: a.input, state: a.state, section })),
    "storage.press": (a) => storagePress(core, a.input, a.id),
    "storage.choose": (a) => storageChoose(core, a.id, a.option),
    "storage.edit": (a) => storageEdit(core, a.id, a.value),
    "storage.requestText": (a) => storageRequestText(core, a.request),

    "notifications.plan": (a) => notificationsPlan(core, a.feed, a.seenMs),
    "notifications.view": (a) => seen(notificationsView(core, a.feed, a.nowMs), (view) => record({ kind: "notifications", feed: a.feed, nowMs: a.nowMs, view })),
  };
}

/** What the harness calls to put a Settings or Notifications state on the screen. */
export interface SettingsParityHandle {
  showAbout(input: AboutInput): void;
  showLoginItem(loginItem: LoginItemInput, experiments: Experiments): void;
  showExperiments(experiments: Experiments): void;
  showDevices(input: DevicesInput, now: number, sheets?: { enroll?: EnrollState | null; approve?: ApproveSheetState | null }): void;
  showStorage(input: StorageInput, state: StorageState): void;
  showNotifications(feed: NotificationInput[], nowMs: number): void;
}

export function settingsParityHandle(store: BridgeStore): SettingsParityHandle {
  const f = store.extras;
  return {
    showAbout: (input) => f.showAbout(input),
    showLoginItem: (loginItem, experiments) => f.showLoginItem(loginItem, experiments),
    showExperiments: (experiments) => f.showExperiments(experiments),
    showDevices: (input, now, sheets) => f.showDevices(input, now, sheets),
    showStorage: (input, state) => f.showStorage(input, state),
    showNotifications: (feed, nowMs) => f.showNotifications(feed, nowMs),
  };
}
