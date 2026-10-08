// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Settings beyond General (About, Devices, Storage, Experiments, launch at
 * login) and Notifications, outside React. Like `BridgeStore`, it fetches
 * from the adapter, derives through the app core (`settings-derive.ts`) and
 * reacts to the host's events; the SwiftUI app's `UpdatesModel`,
 * `DevicesModel`, `StorageModel` and `PersistentModel` do the same natively.
 * The hooks in `settings-hooks.ts` read it.
 */

import { isUnsupported, type DataAdapter } from "./adapter";
import type { CoreClient } from "./core";
import type {
  ApproveSheetState,
  ApproveSheetView,
  DeviceInput,
  DevicesInput,
  DevicesView,
  EnrollMethod,
  EnrollState,
  EnrollView,
} from "./contracts/devices";
import type { SettingsPage, SettingsRow, SettingsSection } from "./contracts/host";
import type { NotificationInput, NotificationsView, SystemNote } from "./contracts/notifications";
import type {
  AboutInput,
  AboutView,
  DriveCheckInput,
  Experiments,
  LoginItemInput,
  LoginItemReport,
  StorageAction,
  StorageInput,
  StorageRequest,
  StorageState,
} from "./contracts/settings";
import type { HostEvent, SettingsValues } from "./protocol";
import {
  aboutView,
  approveOpen,
  approveReduce,
  approveView,
  chooseExperiment,
  cleanDeviceName,
  devicesView,
  enrollInitial,
  enrollReduce,
  enrollView,
  experimentsPage,
  loginItemRows,
  notificationsPlan,
  notificationsView,
  storageChoose,
  storageEdit,
  storageInitial,
  storagePress,
  storageReduce,
  storageSection,
} from "./settings-derive";

export type FeatureName = "about" | "experiments" | "loginItem" | "devices" | "storage" | "notifications";

export interface FeatureResource<T> {
  data: T | undefined;
  isLoading: boolean;
  error: Error | null;
  /** The host has no API for it yet. */
  unsupported: boolean;
}

export interface AboutData {
  input: AboutInput;
  /** The core's About pane; null without the core. */
  view: AboutView | null;
  /** The host can change the update controls. */
  canChange: boolean;
}

export interface ExperimentsData {
  experiments: Experiments;
  /** The core's Experiments tab; null without the core. */
  page: SettingsPage | null;
}

export interface LoginItemData {
  input: LoginItemInput;
  /** General's launch-at-login rows from the core (the toggle and its line). */
  rows: SettingsRow[] | null;
}

export interface DevicesData {
  input: DevicesInput;
  view: DevicesView | null;
  /** The enroll sheet, while open. */
  enroll: { state: EnrollState; view: EnrollView | null } | null;
  /** The approval sheet, while open. */
  approve: { state: ApproveSheetState; view: ApproveSheetView | null } | null;
  /** A row action is running. */
  busy: boolean;
  /** The last row action's failure. */
  actionError: string | null;
  /** Unix seconds the view was drawn at. */
  now: number;
}

export interface StorageData {
  input: StorageInput;
  state: StorageState;
  /** The core's Storage section; null without the core. */
  section: SettingsSection | null;
}

export interface NotificationsData {
  feed: NotificationInput[];
  view: NotificationsView | null;
  unread: number;
}

export interface FeatureOptions {
  now: () => number;
  /** The General settings the store has, for hosts without `loginItem.get`. */
  settingsValues: () => SettingsValues | undefined;
  /** Starts a sign-in through the store (the enroll sheet's Sign in again). */
  signIn: () => Promise<unknown>;
  updateSetting: <K extends keyof SettingsValues>(key: K, value: SettingsValues[K]) => Promise<void>;
  /** Where the notifications marker lives (null: in memory). */
  storage?: Pick<Storage, "getItem" | "setItem"> | null;
}

const NOTIFICATIONS_POLL_MS = 5_000;
const STORAGE_POLL_MS = 2_000;
const ENROLL_POLL_MS = 3_000;
const ENROLL_POLL_LIMIT = 200;
const SIGN_IN_TIMEOUT_MS = 5 * 60_000;
const SEEN_KEY = "cua-spaces:notifications-seen";
/** This app's own entries (a Space ready, a create failed): the daemon's
 * feed only carries agents' notes, so these are kept here, newest last. */
const LOCAL_KEY = "cua-spaces:notifications-local";
const LOCAL_LIMIT = 50;

function readLocalNotes(storage: FeatureOptions["storage"]): NotificationInput[] {
  try {
    const parsed: unknown = JSON.parse(storage?.getItem(LOCAL_KEY) ?? "[]");
    return Array.isArray(parsed) ? (parsed as NotificationInput[]).filter((n) => n && typeof n.id === "string" && typeof n.atMs === "number") : [];
  } catch {
    return [];
  }
}

const idle = <T>(): FeatureResource<T> => ({ data: undefined, isLoading: false, error: null, unsupported: false });
const toError = (e: unknown) => (e instanceof Error ? e : new Error(String(e)));
const message = (e: unknown) => toError(e).message;

/** What a failed adopt reads as (the Tauri app's `failedCheck`). */
const failedCheck = (detail: string): DriveCheckInput => ({ ok: false, reachable: false, authorized: false, versioning: false, detail, applied: false });

const EMPTY_STORAGE_STATE: StorageState = { form: {}, dirty: false, busy: false, request: null, check: null, error: null };

export class SettingsFeature {
  private readonly adapter: DataAdapter;
  private readonly core: CoreClient;
  private readonly options: FeatureOptions;
  private resources: { [K in FeatureName]: FeatureResource<unknown> } = {
    about: idle(),
    experiments: idle(),
    loginItem: idle(),
    devices: idle(),
    storage: idle(),
    notifications: idle(),
  };
  private listeners = new Set<() => void>();
  private ensured = new Set<FeatureName>();
  /** Shown by the parity harness: host answers no longer replace them. */
  private pinned = new Set<FeatureName>();
  private inflight = new Map<FeatureName, Promise<void>>();
  private postListeners = new Set<(note: SystemNote) => void>();
  private timers = new Map<string, ReturnType<typeof setInterval>>();
  private unsubscribe: () => void;
  private disposed = false;

  private about: AboutInput | undefined;
  private aboutCanChange = true;
  private experiments: Experiments = {};
  private loginItem: LoginItemInput | undefined;
  private devices: DevicesInput | undefined;
  private devicesNow: number | undefined;
  private enroll: EnrollState | null = null;
  private approve: ApproveSheetState | null = null;
  private devicesBusy = false;
  private devicesError: string | null = null;
  /** Re-verifications the user put off ("Not Now"). */
  private dismissed = new Set<string>();
  private storageInput: StorageInput | undefined;
  private storageState: StorageState;
  private feed: NotificationInput[] = [];
  private feedNow: number | undefined;
  private local: NotificationInput[];
  private seenMs: number;

  constructor(adapter: DataAdapter, core: CoreClient, options: FeatureOptions) {
    this.adapter = adapter;
    this.core = core;
    this.options = options;
    this.storageState = storageInitial(core) ?? EMPTY_STORAGE_STATE;
    this.seenMs = Number(options.storage?.getItem(SEEN_KEY) ?? 0) || 0;
    this.local = readLocalNotes(options.storage);
    this.unsubscribe = adapter.subscribe((e) => this.onEvent(e));
  }

  /* ---- subscription ---- */

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  get<T>(name: FeatureName): FeatureResource<T> {
    return this.resources[name] as FeatureResource<T>;
  }

  /** System notifications to show (the core's plan); returns the unsubscribe. */
  onPost(listener: (note: SystemNote) => void): () => void {
    this.postListeners.add(listener);
    return () => this.postListeners.delete(listener);
  }

  private set(name: FeatureName, next: Partial<FeatureResource<unknown>>): void {
    this.resources = { ...this.resources, [name]: { ...this.resources[name], ...next } };
    for (const l of [...this.listeners]) l();
  }

  ensure(name: FeatureName): void {
    if (this.ensured.has(name) || this.disposed) return;
    this.ensured.add(name);
    // The launch-at-login line and Storage's place follow the experiments.
    if (name === "loginItem" || name === "storage") this.ensure("experiments");
    if (name === "notifications") this.every("notifications", NOTIFICATIONS_POLL_MS, () => this.refresh("notifications"));
    void this.refresh(name);
  }

  refresh(name: FeatureName): Promise<void> {
    const running = this.inflight.get(name);
    if (running) return running;
    const p = this.load(name).finally(() => this.inflight.delete(name));
    this.inflight.set(name, p);
    return p;
  }

  private async load(name: FeatureName): Promise<void> {
    if (this.pinned.has(name)) return;
    this.set(name, { isLoading: this.resources[name].data === undefined, error: null });
    try {
      // Read first, then apply unless the parity harness pinned it meanwhile.
      const pinned = () => this.pinned.has(name);
      switch (name) {
        case "about": {
          const about = await this.adapter.call("about.get", {});
          if (pinned()) return;
          this.about = about;
          break;
        }
        case "experiments": {
          const experiments = await this.adapter.call("experiments.get", {});
          if (pinned()) return;
          this.experiments = experiments;
          break;
        }
        case "loginItem": {
          const report = await this.loginReport();
          if (pinned()) return;
          this.loginItem = { ...report, busy: false, error: this.loginItem?.error ?? null };
          break;
        }
        case "devices": {
          const devices = await this.adapter.call("devices.get", {});
          if (pinned()) return;
          this.devices = devices;
          break;
        }
        case "storage": {
          const input = await this.adapter.call("storage.get", {});
          if (pinned()) return;
          this.storageInput = input;
          if (input.storage) this.storageSend({ type: "loaded", storage: input.storage });
          break;
        }
        case "notifications": {
          const feed = await this.adapter.call("notifications.list", {});
          if (pinned()) return;
          this.feed = feed;
          this.announce();
          break;
        }
      }
      this.publish(name);
    } catch (e) {
      if (this.disposed) return;
      this.set(name, { isLoading: false, error: isUnsupported(e) ? null : toError(e), unsupported: isUnsupported(e) });
    }
  }

  /** Launch at login from the host, or from the General value on hosts without `loginItem.get`. */
  private async loginReport(): Promise<LoginItemReport> {
    try {
      return await this.adapter.call("loginItem.get", {});
    } catch (e) {
      if (!isUnsupported(e)) throw e;
      const on = this.options.settingsValues()?.launchAtLogin;
      if (on === null || on === undefined) throw e;
      return { status: on ? "enabled" : "notRegistered", providesSpaces: false, runsAgents: false };
    }
  }

  private publish(name: FeatureName): void {
    const done = (data: unknown) => this.set(name, { data, isLoading: false, error: null, unsupported: false });
    switch (name) {
      case "about":
        if (this.about) done({ input: this.about, view: aboutView(this.core, this.about), canChange: this.aboutCanChange } satisfies AboutData);
        break;
      case "experiments":
        done({ experiments: this.experiments, page: experimentsPage(this.core, this.experiments) } satisfies ExperimentsData);
        // What follows the switches.
        if (this.resources.loginItem.data !== undefined) this.publish("loginItem");
        break;
      case "loginItem":
        if (this.loginItem) done({ input: this.loginItem, rows: loginItemRows(this.core, this.loginItem, this.experiments) } satisfies LoginItemData);
        break;
      case "devices": {
        if (!this.devices) break;
        const now = this.devicesNow ?? Math.floor(this.options.now() / 1000);
        const list = this.devices.devices ?? [];
        done({
          input: this.devices,
          view: devicesView(this.core, this.devices, now),
          enroll: this.enroll ? { state: this.enroll, view: enrollView(this.core, this.enroll) } : null,
          approve: this.approve ? { state: this.approve, view: approveView(this.core, this.approve, list) } : null,
          busy: this.devicesBusy,
          actionError: this.devicesError,
          now,
        } satisfies DevicesData);
        break;
      }
      case "storage":
        if (this.storageInput) {
          const section = storageSection(this.core, this.storageInput, this.storageState);
          done({ input: this.storageInput, state: this.storageState, section } satisfies StorageData);
          this.watchStorage(section);
        }
        break;
      case "notifications": {
        const feed = this.pinned.has("notifications") ? this.feed : [...this.feed, ...this.local];
        const view = notificationsView(this.core, feed, this.feedNow ?? this.options.now());
        done({ feed, view, unread: view?.unread ?? feed.filter((n) => !n.read).length } satisfies NotificationsData);
        break;
      }
    }
  }

  private every(key: string, ms: number, f: () => unknown): void {
    if (this.timers.has(key) || this.disposed) return;
    this.timers.set(key, setInterval(() => void f(), ms));
  }

  private stop(key: string): void {
    const t = this.timers.get(key);
    if (t) clearInterval(t);
    this.timers.delete(key);
  }

  /* ---- About ---- */

  async setAbout(patch: { autoCheck?: boolean; autoInstall?: boolean; channel?: AboutInput["channel"] }): Promise<void> {
    const before = this.about;
    if (before) {
      this.about = { ...before, ...patch };
      this.publish("about");
    }
    try {
      this.about = await this.adapter.call("about.set", patch);
      this.publish("about");
    } catch (e) {
      this.about = before;
      if (isUnsupported(e)) this.aboutCanChange = false;
      this.publish("about");
      throw toError(e);
    }
  }

  async checkNow(): Promise<void> {
    if (this.about) {
      this.about = { ...this.about, checking: true };
      this.publish("about");
    }
    try {
      this.about = await this.adapter.call("about.checkNow", {});
      this.publish("about");
      // Follow the check until the host says it ended.
      if (this.about.checking) this.every("about", 1_000, () => this.followCheck());
    } catch (e) {
      if (this.about) this.about = { ...this.about, checking: false };
      this.publish("about");
      throw toError(e);
    }
  }

  private async followCheck(): Promise<void> {
    await this.refresh("about");
    if (!this.about?.checking) this.stop("about");
  }

  /* ---- Experiments ---- */

  async chooseExperiment(row: string, option: string): Promise<void> {
    const before = this.experiments;
    const next = chooseExperiment(this.core, before, row, option);
    if (JSON.stringify(next) === JSON.stringify(before)) return;
    this.experiments = next;
    this.publish("experiments");
    try {
      this.experiments = await this.adapter.call("experiments.set", { experiments: next });
      this.publish("experiments");
    } catch (e) {
      this.experiments = before;
      this.publish("experiments");
      throw toError(e);
    }
  }

  /* ---- Launch at login ---- */

  async setLaunchAtLogin(on: boolean): Promise<void> {
    const current = this.loginItem;
    if (!current || current.busy) return;
    this.loginItem = { ...current, busy: true, error: null };
    this.publish("loginItem");
    try {
      let report: LoginItemReport;
      try {
        report = await this.adapter.call("loginItem.set", { on });
      } catch (e) {
        if (!isUnsupported(e)) throw e;
        await this.options.updateSetting("launchAtLogin", on);
        report = await this.loginReport();
      }
      this.loginItem = { ...report, busy: false, error: null };
    } catch (e) {
      this.loginItem = { ...current, busy: false, error: message(e) };
    }
    this.publish("loginItem");
  }

  async openLoginItems(): Promise<void> {
    await this.adapter.call("loginItem.openSettings", {});
  }

  /* ---- Devices ---- */

  private devicesList(): DeviceInput[] {
    return this.devices?.devices ?? [];
  }

  private currentDevicesView(): DevicesView | null {
    return this.devices ? devicesView(this.core, this.devices, this.devicesNow ?? Math.floor(this.options.now() / 1000)) : null;
  }

  startEnroll(): void {
    this.enroll = enrollInitial(this.core);
    this.publish("devices");
  }

  closeEnroll(): void {
    this.enroll = null;
    this.publish("devices");
  }

  backEnroll(): void {
    if (!this.enroll) return;
    this.enroll = enrollReduce(this.core, this.enroll, { type: "back" });
    this.publish("devices");
  }

  /** "Sign in again" or "Approve from another device" (`DevicesModel.chooseEnroll`). */
  async chooseEnroll(method: EnrollMethod): Promise<void> {
    const send = (action: Parameters<typeof enrollReduce>[2]) => {
      if (!this.enroll) return;
      this.enroll = enrollReduce(this.core, this.enroll, action);
      this.publish("devices");
    };
    // Read fresh after each await: the sheet may have moved or closed.
    const phase = (): EnrollState["phase"] | undefined => this.enroll?.phase;
    send({ type: "choose", method });
    if (phase() === "signing-in") {
      const ok = await this.signInAgain();
      if (phase() !== "signing-in") return;
      if (!ok) return send({ type: "failed", error: "Sign-in did not finish." });
      send({ type: "signed-in" });
    }
    if (phase() !== "registering") return;
    try {
      const r = await this.adapter.call("devices.enroll", {});
      send({ type: "registered", enrolled: r.enrolled, code: r.code });
      if (this.devices && !r.enrolled) this.devices = { ...this.devices, pendingCode: r.code };
    } catch (e) {
      return send({ type: "failed", error: message(e) });
    }
    for (let i = 0; phase() === "waiting" && i < ENROLL_POLL_LIMIT && !this.disposed; i++) {
      if (await this.adapter.call("devices.checkEnrolled", {}).catch(() => false)) {
        send({ type: "approved" });
        break;
      }
      await new Promise((r) => setTimeout(r, ENROLL_POLL_MS));
    }
    await this.refresh("devices");
  }

  /** Starts a sign-in and waits for it to finish or fail. */
  private signInAgain(): Promise<boolean> {
    return new Promise<boolean>((resolve) => {
      const timer = setTimeout(() => finish(false), SIGN_IN_TIMEOUT_MS);
      const off = this.adapter.subscribe((e) => {
        if (e.type === "session.signedIn") finish(true);
        if (e.type === "session.signInFailed") finish(false);
      });
      function finish(ok: boolean) {
        clearTimeout(timer);
        off();
        resolve(ok);
      }
      this.options.signIn().catch(() => finish(false));
    });
  }

  /** The row's Approve… (the prompt for that device). */
  openApproval(deviceId: string): void {
    const prompt = this.currentDevicesView()?.approvals.find((p) => p.deviceId === deviceId);
    if (!prompt) return;
    this.approve = approveOpen(this.core, prompt);
    this.publish("devices");
  }

  closeApproval(): void {
    this.approve = null;
    this.publish("devices");
  }

  setApprovalCode(code: string): void {
    if (!this.approve) return;
    this.approve = approveReduce(this.core, this.approve, { type: "set-code", code }, this.devicesList());
    this.publish("devices");
  }

  /** Approve: the host asks for presence, then the relay (`DevicesModel.approve`). */
  async approveDevice(): Promise<void> {
    const s = this.approve;
    if (!s) return;
    const list = this.devicesList();
    const next = approveReduce(this.core, s, { type: "submit" }, list);
    const request = approveView(this.core, next, list)?.request;
    if (!next.busy || !request) return;
    this.approve = next;
    this.publish("devices");
    try {
      await this.adapter.call("devices.approve", { code: request.code, deviceId: request.deviceId });
    } catch (e) {
      if (this.approve) this.approve = approveReduce(this.core, this.approve, { type: "failed", error: message(e) }, list);
      this.publish("devices");
      return;
    }
    this.approve = null;
    await this.refresh("devices");
  }

  /** The sheet's Deny (revokes a new device) or Not Now (re-verification). */
  async denyApproval(): Promise<void> {
    const s = this.approve;
    if (!s) return;
    const view = approveView(this.core, s, this.devicesList());
    this.approve = null;
    this.publish("devices");
    if (!view?.denyRevokes) {
      this.dismissed.add(s.deviceId);
      return;
    }
    await this.deviceAction(() => this.adapter.call("devices.revoke", { id: s.deviceId }));
  }

  /** The row's Deny: revokes a pending device, or puts off a re-verification. */
  async denyDevice(deviceId: string): Promise<void> {
    const prompt = this.currentDevicesView()?.approvals.find((p) => p.deviceId === deviceId);
    if (!prompt) return;
    if (prompt.expired) {
      this.dismissed.add(deviceId);
      this.publish("devices");
      return;
    }
    await this.deviceAction(() => this.adapter.call("devices.revoke", { id: deviceId }));
  }

  isDismissed(deviceId: string): boolean {
    return this.dismissed.has(deviceId);
  }

  renameDevice(id: string, name: string): Promise<void> {
    const clean = cleanDeviceName(this.core, name);
    if (!clean) return Promise.resolve();
    return this.deviceAction(() => this.adapter.call("devices.rename", { id, name: clean }));
  }

  revokeDevice(id: string): Promise<void> {
    return this.deviceAction(() => this.adapter.call("devices.revoke", { id }));
  }

  confirmMachine(id: string): Promise<void> {
    return this.deviceAction(() => this.adapter.call("devices.confirmMachine", { id }));
  }

  private async deviceAction(f: () => Promise<unknown>): Promise<void> {
    this.devicesBusy = true;
    this.devicesError = null;
    this.publish("devices");
    try {
      await f();
    } catch (e) {
      this.devicesError = message(e);
    }
    this.devicesBusy = false;
    await this.refresh("devices");
    this.publish("devices");
  }

  /* ---- Storage ---- */

  /** A row's button, choice or field, through the core (`StorageModel`). */
  storagePress(rowId: string): void {
    if (this.storageInput) this.storageSend(storagePress(this.core, this.storageInput, rowId));
  }

  storageChoose(rowId: string, option: string): void {
    this.storageSend(storageChoose(this.core, rowId, option));
  }

  storageEdit(rowId: string, value: string): void {
    this.storageSend(storageEdit(this.core, rowId, value));
  }

  /** The section's Save. */
  storageSave(): void {
    this.storageSend({ type: "save" });
  }

  private storageSend(action: StorageAction | null): void {
    if (!action) return;
    const before = this.storageState;
    this.storageState = storageReduce(this.core, before, action);
    if (this.storageInput) this.publish("storage");
    if (!before.busy && this.storageState.busy && this.storageState.request) void this.runStorage(this.storageState.request);
  }

  private async runStorage(request: StorageRequest): Promise<void> {
    const settle = (action: StorageAction) => {
      this.storageState = storageReduce(this.core, this.storageState, action);
      this.publish("storage");
    };
    try {
      const check = await this.adapter.call("storage.run", { request });
      if (request.kind === "test") settle({ type: "checked", check: check! });
      else if (request.kind === "save") settle({ type: "saved", check: check! });
      else if (request.kind === "adopt") settle({ type: "adopted", check: check! });
      else settle({ type: "done" });
    } catch (e) {
      if (request.kind === "adopt") settle({ type: "adopted", check: failedCheck(message(e)) });
      else settle({ type: "failed", error: message(e) });
    }
    await this.refresh("storage");
  }

  /** While the mount settles or the agent prompt shows, ask again every 2 s. */
  private watchStorage(section: SettingsSection | null): void {
    const rows = section?.rows ?? [];
    const settling = rows.some((r) => r.id === "mount-approval" || r.id === "s3-prompt" || (r.id === "mount-path" && !r.button));
    if (settling && !this.pinned.has("storage")) this.every("storage", STORAGE_POLL_MS, () => this.refresh("storage"));
    else this.stop("storage");
  }

  /* ---- Notifications ---- */

  /** Posts what the core plans (each entry once, none of the first backlog). */
  private announce(): void {
    const plan = notificationsPlan(this.core, this.feed, this.seenMs);
    if (!plan) return;
    for (const note of plan.post) for (const l of [...this.postListeners]) l(note);
    if (plan.seenMs !== this.seenMs) {
      this.seenMs = plan.seenMs;
      this.options.storage?.setItem(SEEN_KEY, String(plan.seenMs));
    }
  }

  async markAllRead(): Promise<void> {
    if (this.local.some((n) => !n.read)) {
      this.local = this.local.map((n) => ({ ...n, read: true }));
      this.saveLocal();
    }
    await this.adapter.call("notifications.markAllRead", {});
    await this.refresh("notifications");
  }

  /** Adds one of this app's own entries (a Space became ready, a create
   * failed and why) to the list, so a missed toast can still be read. */
  record(note: { kind: "message" | "error"; title: string; body?: string }): void {
    const atMs = this.options.now();
    const id = `app:${atMs}:${Math.random().toString(36).slice(2, 8)}`;
    this.local = [...this.local, { id, atMs, agent: null, kind: note.kind, title: note.title, body: note.body ?? "", read: false }].slice(-LOCAL_LIMIT);
    this.saveLocal();
    if (this.resources.notifications.data !== undefined) this.publish("notifications");
  }

  private saveLocal(): void {
    try {
      this.options.storage?.setItem(LOCAL_KEY, JSON.stringify(this.local));
    } catch {
      // Storage full or unavailable: the list still holds them until reload.
    }
  }

  /* ---- parity harness (parity.ts) ---- */

  showAbout(input: AboutInput): void {
    this.pin("about");
    this.about = input;
    this.publish("about");
  }

  showExperiments(experiments: Experiments): void {
    this.pin("experiments");
    this.experiments = experiments;
    this.publish("experiments");
  }

  showLoginItem(input: LoginItemInput, experiments: Experiments = {}): void {
    this.pin("loginItem");
    this.pin("experiments");
    this.experiments = experiments;
    this.loginItem = input;
    this.publish("experiments");
    this.publish("loginItem");
  }

  /** The Devices page at `now` (Unix seconds), with a sheet open or none. */
  showDevices(input: DevicesInput, now: number, sheets: { enroll?: EnrollState | null; approve?: ApproveSheetState | null } = {}): void {
    this.pin("devices");
    this.devices = input;
    this.devicesNow = now;
    this.enroll = sheets.enroll ?? null;
    this.approve = sheets.approve ?? null;
    this.publish("devices");
  }

  showStorage(input: StorageInput, state: StorageState, experiments: Experiments = { cuaVolume: true }): void {
    this.pin("storage");
    this.pin("experiments");
    this.experiments = experiments;
    this.storageInput = input;
    this.storageState = state;
    this.publish("experiments");
    this.publish("storage");
  }

  showNotifications(feed: NotificationInput[], nowMs: number): void {
    this.pin("notifications");
    this.feed = feed;
    this.feedNow = nowMs;
    this.publish("notifications");
  }

  private pin(name: FeatureName): void {
    this.ensured.add(name);
    this.pinned.add(name);
    this.stop(name);
  }

  /* ---- events ---- */

  private onEvent(e: HostEvent): void {
    if (this.disposed) return;
    const reload = (...names: FeatureName[]) => {
      for (const n of names) if (this.ensured.has(n)) void this.refresh(n);
    };
    switch (e.type) {
      case "settings.changed":
        reload("about", "experiments", "loginItem");
        break;
      case "session.signedIn":
      case "session.signedOut":
      case "session.changed":
        reload("devices");
        break;
      case "agents.changed":
        reload("notifications", "loginItem");
        break;
      case "machines.changed":
        reload("devices", "loginItem");
        break;
    }
  }

  dispose(): void {
    this.disposed = true;
    this.unsubscribe();
    for (const t of this.timers.values()) clearInterval(t);
    this.timers.clear();
    this.listeners.clear();
    this.postListeners.clear();
  }
}

/** The feature for a bridge store: its adapter, core and General settings. */
export function createSettingsFeature(
  store: {
    adapter: DataAdapter;
    core: CoreClient;
    get<T>(name: "settings"): { data: T | undefined };
    signIn(): Promise<unknown>;
    updateSetting<K extends keyof SettingsValues>(key: K, value: SettingsValues[K]): Promise<void>;
  },
  now: () => number,
): SettingsFeature {
  return new SettingsFeature(store.adapter, store.core, {
    now,
    settingsValues: () => store.get<{ values: SettingsValues }>("settings").data?.values,
    signIn: () => store.signIn(),
    updateSetting: (key, value) => store.updateSetting(key, value),
    storage: typeof localStorage === "undefined" ? null : localStorage,
  });
}
