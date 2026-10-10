// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Hooks for Settings beyond General and for Notifications. Each returns
 * `{ data, isLoading, error, unsupported, refresh }` plus its actions, like
 * the hooks in `hooks.ts`; `unsupported` says the host has no API for it yet.
 */

import { useContext, useEffect, useSyncExternalStore } from "react";
import { BridgeContext, type BridgeContextValue } from "./BridgeProvider";
import type { EnrollMethod } from "./contracts/devices";
import type { SystemNote } from "./contracts/notifications";
import type { AboutInput } from "./contracts/settings";
import type {
  AboutData,
  DevicesData,
  ExperimentsData,
  FeatureName,
  FeatureResource,
  LoginItemData,
  NotificationsData,
  SettingsFeature,
  StorageData,
} from "./settings-feature";

export type { AboutData, DevicesData, ExperimentsData, FeatureResource, LoginItemData, NotificationsData, StorageData } from "./settings-feature";

function useCtx(): BridgeContextValue {
  const ctx = useContext(BridgeContext);
  if (!ctx) throw new Error("bridge hooks need a <BridgeProvider> above them");
  return ctx;
}

const LOADING: FeatureResource<never> = { data: undefined, isLoading: true, error: null, unsupported: false };
const noopSubscribe = () => () => {};

type Hook<T> = FeatureResource<T> & { refresh(): Promise<void> };

function useFeature<T>(name: FeatureName): Hook<T> {
  const { store, ready } = useCtx();
  const feature = store?.extras;
  useEffect(() => {
    feature?.ensure(name);
  }, [feature, name]);
  const r = useSyncExternalStore(
    feature ? feature.subscribe : noopSubscribe,
    () => (feature ? feature.get<T>(name) : (LOADING as FeatureResource<T>)),
    () => LOADING as FeatureResource<T>,
  );
  const shown = feature && r.data === undefined && !r.error && !r.unsupported ? { ...r, isLoading: true } : r;
  return { ...shown, refresh: () => ready.then((s) => s.extras.refresh(name)) };
}

function useRun(): <R>(f: (s: SettingsFeature) => Promise<R> | R) => Promise<R> {
  const { ready } = useCtx();
  return (f) => ready.then((s) => f(s.extras));
}

export interface AboutHook extends Hook<AboutData> {
  setAbout(patch: { autoCheck?: boolean; autoInstall?: boolean; channel?: AboutInput["channel"] }): Promise<void>;
  checkNow(): Promise<void>;
}

/** Settings, About: the core's pane over the host's version and updater. */
export function useAbout(): AboutHook {
  const r = useFeature<AboutData>("about");
  const run = useRun();
  return { ...r, setAbout: (p) => run((f) => f.setAbout(p)), checkNow: () => run((f) => f.checkNow()) };
}

export interface ExperimentsHook extends Hook<ExperimentsData> {
  /** A switch's choice (`on` / `off`), by row id (`experiment:cua_volume`). */
  chooseExperiment(row: string, option: string): Promise<void>;
}

export function useExperiments(): ExperimentsHook {
  const r = useFeature<ExperimentsData>("experiments");
  const run = useRun();
  return { ...r, chooseExperiment: (row, option) => run((f) => f.chooseExperiment(row, option)) };
}

export interface LoginItemHook extends Hook<LoginItemData> {
  setLaunchAtLogin(on: boolean): Promise<void>;
  /** System Settings, Login Items. */
  openLoginItems(): Promise<void>;
}

/** Launch at login: what the system reports, and the core's rows for it. */
export function useLoginItem(): LoginItemHook {
  const r = useFeature<LoginItemData>("loginItem");
  const run = useRun();
  return { ...r, setLaunchAtLogin: (on) => run((f) => f.setLaunchAtLogin(on)), openLoginItems: () => run((f) => f.openLoginItems()) };
}

export interface DevicesHook extends Hook<DevicesData> {
  startEnroll(): void;
  chooseEnroll(method: EnrollMethod): Promise<void>;
  backEnroll(): void;
  closeEnroll(): void;
  openApproval(deviceId: string): void;
  setApprovalCode(code: string): void;
  approve(): Promise<void>;
  /** The sheet's Deny or Not Now. */
  deny(): Promise<void>;
  closeApproval(): void;
  /** The row's Deny. */
  denyDevice(deviceId: string): Promise<void>;
  rename(id: string, name: string): Promise<void>;
  revoke(id: string): Promise<void>;
  confirmMachine(id: string): Promise<void>;
}

/** Settings, Devices: this device's enrollment, the account's devices, the access log and both sheets. */
export function useDevices(): DevicesHook {
  const r = useFeature<DevicesData>("devices");
  const run = useRun();
  const now = <R>(f: (s: SettingsFeature) => R) => void run(f);
  return {
    ...r,
    startEnroll: () => now((f) => f.startEnroll()),
    chooseEnroll: (m) => run((f) => f.chooseEnroll(m)),
    backEnroll: () => now((f) => f.backEnroll()),
    closeEnroll: () => now((f) => f.closeEnroll()),
    openApproval: (id) => now((f) => f.openApproval(id)),
    setApprovalCode: (code) => now((f) => f.setApprovalCode(code)),
    approve: () => run((f) => f.approveDevice()),
    deny: () => run((f) => f.denyApproval()),
    closeApproval: () => now((f) => f.closeApproval()),
    denyDevice: (id) => run((f) => f.denyDevice(id)),
    rename: (id, name) => run((f) => f.renameDevice(id, name)),
    revoke: (id) => run((f) => f.revokeDevice(id)),
    confirmMachine: (id) => run((f) => f.confirmMachine(id)),
  };
}

export interface StorageHook extends Hook<StorageData> {
  press(rowId: string): void;
  choose(rowId: string, option: string): void;
  edit(rowId: string, value: string): void;
  save(): void;
}

/** Settings, Storage: the Cua Volume's storage, mount and cache, as the core lays them out. */
export function useStorageSettings(): StorageHook {
  const r = useFeature<StorageData>("storage");
  const run = useRun();
  const now = <R>(f: (s: SettingsFeature) => R) => void run(f);
  return {
    ...r,
    press: (id) => now((f) => f.storagePress(id)),
    choose: (id, option) => now((f) => f.storageChoose(id, option)),
    edit: (id, value) => now((f) => f.storageEdit(id, value)),
    save: () => now((f) => f.storageSave()),
  };
}

export interface NotificationsHook extends Hook<NotificationsData> {
  markAllRead(): Promise<void>;
}

/** The daemon's notifications feed, polled every 5 s while the app is open. */
export function useNotifications(): NotificationsHook {
  const r = useFeature<NotificationsData>("notifications");
  const run = useRun();
  return { ...r, markAllRead: () => run((f) => f.markAllRead()) };
}

/** Calls `listener` for each entry the core says to announce. */
export function useNotificationPosts(listener: (note: SystemNote) => void): void {
  const { store } = useCtx();
  useEffect(() => store?.extras.onPost(listener), [store, listener]);
}
