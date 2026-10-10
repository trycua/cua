// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// This machine, its devices and the system for the host area's tests (the
// SwiftUI app's FixtureHost, ScriptedHost, FakeAccountTokens,
// FixtureDevices, FixturePresence and FixtureLoginItem). In memory: no
// service, launchd, network, notification, login item or keychain.
import type { DeviceEnrollment, DevicesLike, DevicesSnapshot, HostLike, HostSettingsChange, HostSetupOptions, HostStatus, RelayDevice } from "../src/native/generated/index";
import type { SystemNote, SystemServices } from "../src/bridge/system";
import type { HostModel } from "../src/model/host";
import { FixtureLoginItem } from "../src/model/login-item";
import { FixtureUpdater } from "../src/model/updates";
import { unconfiguredHost } from "./fixtures";

/** An SDK error as the bindings throw it (`CuaError.<tag>: message`). */
export function sdkError(tag: string, message: string): Error {
  const e = new Error(`CuaError.${tag}: ${message}`);
  return Object.assign(e, { [Symbol.for("typeName")]: "CuaError", tag });
}

/** An in-memory host: setup succeeds (or fails with the script), sharing toggles. */
export class FixtureHost {
  current: HostStatus = unconfiguredHost();
  readonly calls: string[] = [];
  /** Setup's token each time. */
  readonly tokens: (string | undefined)[] = [];
  /** The next setups fail with these first. */
  script: Error[] = [];
  /** The next `configure` fails with this (once). */
  failConfigure: string | null = null;
  owner = "user-1";
  ownerEmail = "ada@example.com";

  get like(): HostLike {
    return this as unknown as HostLike;
  }

  async status() {
    return this.current;
  }

  async setup(o: HostSetupOptions, token: string | undefined) {
    this.tokens.push(token);
    this.calls.push(`setup:${o.mode}:${o.direct ?? o.relayUrl ?? "https://relay.cua.ai"}`);
    const next = this.script.shift();
    if (next) throw next;
    const relay = o.mode === "relay";
    const spare = o.profile === "spare";
    this.current = {
      ...unconfiguredHost(),
      configured: true,
      mode: o.mode,
      relayUrl: relay ? (o.relayUrl ?? "https://relay.cua.ai") : undefined,
      directUrl: relay ? undefined : `http://${o.direct ?? ""}`,
      machineId: "0123abcd4567",
      name: o.name ?? "This machine",
      sharing: true,
      serviceInstalled: true,
      serviceRunning: true,
      serviceKind: "launchd",
      online: true,
      allow: o.allow,
      shareDesktop: o.shareDesktop ?? !spare,
      provideSpaces: o.provideSpaces ?? spare,
      owner: relay ? this.owner : undefined,
      ownerEmail: relay ? this.ownerEmail : undefined,
    };
    return this.current;
  }

  async configure(change: HostSettingsChange) {
    this.calls.push(`configure:${change.shareDesktop ?? "-"}:${change.provideSpaces ?? "-"}`);
    if (this.failConfigure) {
      const why = this.failConfigure;
      this.failConfigure = null;
      throw sdkError("Runtime", why);
    }
    if (change.shareDesktop !== undefined) this.current = { ...this.current, shareDesktop: change.shareDesktop };
    if (change.provideSpaces !== undefined) this.current = { ...this.current, provideSpaces: change.provideSpaces };
    // Both off: nothing to share, so sharing stops (as the host does).
    if (!this.current.shareDesktop && !this.current.provideSpaces) this.current = { ...this.current, sharing: false, clients: [] };
    return this.current;
  }

  async stopSharing() {
    this.calls.push("stop");
    this.current = { ...this.current, sharing: false, clients: [] };
    return this.current;
  }

  async startSharing() {
    this.calls.push("start");
    this.current = { ...this.current, sharing: true, pausedSignedOut: false };
    return this.current;
  }

  async pauseSignedOut() {
    this.calls.push("pause");
    if (!this.current.configured || this.current.mode !== "relay") return this.current;
    this.current = { ...this.current, pausedSignedOut: true, sharing: false, online: false, clients: [], serviceInstalled: false, serviceRunning: false };
    return this.current;
  }

  async resumeSignedIn(account: string) {
    this.calls.push(`resume:${account}`);
    if (!this.current.pausedSignedOut) return this.current;
    if (account !== this.owner && account.toLowerCase() !== this.ownerEmail.toLowerCase()) {
      throw sdkError("InvalidArgument", "this machine is shared with another Cua account");
    }
    this.current = { ...this.current, pausedSignedOut: false, sharing: true, online: true, serviceInstalled: true, serviceRunning: true };
    return this.current;
  }

  async remove() {
    this.calls.push("remove");
    this.current = unconfiguredHost();
  }
}

/** The account as host setup sees it: a token store that can be signed out, offline, or refuse to refresh, and a sign-in that can finish or not. */
export class FakeAccountTokens {
  token: string | null = null;
  /** What a forced refresh yields (null: refused). */
  refreshed: string | null = null;
  failures: Error[] = [];
  /** What a sign-in leaves signed in as (null: cancelled). */
  signInGives: string | null = null;
  readonly reads: boolean[] = [];
  signIns = 0;
  progressDuringSignIn: string | null = null;

  wire(model: HostModel): void {
    model.tokenRetryDelays = [0, 0];
    model.accountToken = async (force) => {
      this.reads.push(force);
      const fail = this.failures.shift();
      if (fail) throw fail;
      if (force) this.token = this.refreshed;
      return this.token;
    };
    model.signIn = async () => {
      this.signIns += 1;
      this.progressDuringSignIn = model.progress;
      this.token = this.signInGives;
      return this.signInGives !== null;
    };
  }
}

/** An account on a relay: this machine enrolled, a new device and an expired one asking, and two machines. */
export class FixtureDevices {
  current: DevicesSnapshot;
  readonly calls: string[] = [];
  enrollEnrolled = false;
  enrolledNow = false;
  failApprove: string | null = null;
  /** What `approve` leaves the device as (the relay may not enroll it). */
  approveState = "enrolled";
  /** `snapshot` fails with this (a device the relay has not enrolled). */
  failSnapshot: string | null = null;

  constructor(now = BigInt(Math.floor(Date.now() / 1000))) {
    const day = 86_400n;
    const device = (id: string, name: string, state: string, platform: string, until?: bigint, seen?: bigint, current = false): RelayDevice => ({
      id,
      name,
      state,
      platform,
      createdAt: now - 40n * day,
      enrolledUntil: until,
      lastSeen: seen,
      current,
    });
    this.current = {
      localDeviceId: "dev_mac",
      devices: [
        device("dev_mac", "MacBook Pro", "enrolled", "macos", now + 24n * day, now - 60n, true),
        device("dev_work", "Work laptop", "pending", "windows"),
        device("dev_old", "Old laptop", "expired", "linux", now - 2n * day, now - 33n * day),
      ],
      enforceAfter: now - day,
      audit: [],
      machineNames: new Map([["m2", "build-box"]]),
      machines: [
        { id: "m2", spaceId: "relay:m2", name: "build-box", ownerId: "me", ownerEmail: undefined, role: "owner", online: false, sharing: true, version: "0.5.0", url: "", allow: [], clients: [], confirmed: false },
      ],
    };
  }

  get like(): DevicesLike {
    return this as unknown as DevicesLike;
  }

  async snapshot() {
    if (this.failSnapshot) throw sdkError("PermissionDenied", this.failSnapshot);
    return this.current;
  }

  async enroll(): Promise<DeviceEnrollment> {
    this.calls.push("enroll");
    const me = this.current.devices.find((d) => d.current)!;
    return { device: me, enrolled: this.enrollEnrolled, code: this.enrollEnrolled ? undefined : "K7QX-M2RP" };
  }

  async checkEnrolled() {
    this.calls.push("check");
    return this.enrolledNow;
  }

  async approve(code: string | undefined, deviceId: string | undefined): Promise<RelayDevice> {
    this.calls.push(`approve:${code ?? deviceId ?? ""}`);
    if (this.failApprove) throw sdkError("PermissionDenied", this.failApprove);
    const i = this.current.devices.findIndex((d) => d.id === deviceId || (code !== undefined && d.state === "pending"));
    const d = { ...this.current.devices[i]!, state: this.approveState };
    this.current = { ...this.current, devices: this.current.devices.map((x, j) => (j === i ? d : x)) };
    return d;
  }

  async rename(id: string, name: string) {
    this.calls.push(`rename:${id}:${name}`);
    this.current = { ...this.current, devices: this.current.devices.map((d) => (d.id === id ? { ...d, name } : d)) };
    return this.current.devices.find((d) => d.id === id)!;
  }

  async revoke(id: string) {
    this.calls.push(`revoke:${id}`);
    this.current = { ...this.current, devices: this.current.devices.map((d) => (d.id === id ? { ...d, state: "revoked" } : d)) };
    return this.current.devices.find((d) => d.id === id)!;
  }

  async confirmMachine(id: string) {
    this.calls.push(`confirm-machine:${id}`);
    this.current = { ...this.current, machines: this.current.machines.map((m) => (m.id === id ? { ...m, confirmed: true } : m)) };
    return this.current.machines.find((m) => m.id === id)!;
  }
}

/** The system in memory: what was opened, shown, posted and asked. */
export class FixtureSystem implements SystemServices {
  readonly opened: string[] = [];
  readonly revealed: string[] = [];
  readonly notes: SystemNote[] = [];
  readonly asked: string[] = [];
  allow = true;
  loginItem = new FixtureLoginItem();
  updater = new FixtureUpdater(Date.UTC(2026, 9, 7, 12, 0));
  constructor(readonly home: string) {}
  async openExternal(url: string) {
    this.opened.push(url);
  }
  reveal(p: string) {
    this.revealed.push(p);
  }
  notify(note: SystemNote) {
    this.notes.push(note);
  }
  async confirmPresence(reason: string) {
    this.asked.push(reason);
    if (!this.allow) throw new Error("Approval was cancelled");
  }
}

/** The bridge methods `host-bridge.test.ts` checks (the shared shape test leaves them to it). */
export const HOST_METHODS = [
  "volume.overview", "volume.storage", "volume.storageSet", "volume.mount", "volume.unmount",
  "volume.approve", "volume.deny", "volume.revoke", "volume.resolve", "volume.reveal",
  "about.get", "about.set", "about.checkNow", "loginItem.get", "loginItem.set", "loginItem.openSettings",
  "devices.get", "devices.enroll", "devices.checkEnrolled", "devices.approve", "devices.rename", "devices.revoke", "devices.confirmMachine",
  "storage.get", "storage.run", "notifications.list", "notifications.markAllRead",
  "host.setUp", "host.action", "host.openSettings", "settings.get", "settings.choose",
  "onboarding.get", "onboarding.complete",
];

/** The page's operations over them (the shared adapter test leaves them to it too). */
export const HOST_OPS = [
  "volume.overview", "volume.storage", "volume.storageSet", "volume.mount", "volume.unmount",
  "volume.approve", "volume.deny", "volume.revoke", "volume.resolve", "volume.reveal",
  "about.get", "about.set", "about.checkNow", "loginItem.get", "loginItem.set", "loginItem.openSettings",
  "devices.get", "devices.enroll", "devices.checkEnrolled", "devices.approve", "devices.rename", "devices.revoke", "devices.confirmMachine",
  "storage.get", "storage.run", "notifications.list", "notifications.markAllRead",
  "host.setUp", "host.action", "host.openSettings", "settings.get", "settings.set", "settings.choose", "experiments.set", "session.completeOnboarding",
];
