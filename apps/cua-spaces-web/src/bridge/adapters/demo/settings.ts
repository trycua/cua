// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The demo host's About, Devices, Storage, Experiments, launch at login and
 * notifications feed: plausible data in memory, shaped like the SwiftUI
 * app's samples (`DevicesModel.sample`), changing as the real host would.
 */

import { HostError } from "../../adapter";
import type { DeviceInput, DevicesInput, PairingCode } from "../../contracts/devices";
import type { MachineRow } from "../../contracts/host";
import type { NotificationInput } from "../../contracts/notifications";
import type { AboutInput, DriveCheckInput, Experiments, LoginItemReport, StorageInput, UpdateState } from "../../contracts/settings";
import { NO_EXPERIMENTS } from "../../contracts/volume";
import { demoMachine, MAC_DEMO_PLATFORM, type DemoPlatform } from "./platform";
import type { NotificationsOperations } from "../../ops/notifications";
import type { SettingsOperations } from "../../ops/settings";
import type { HostEvent, SettingsValues } from "../../protocol";

export interface DemoSettingsContext {
  now: () => number;
  wait: (ms: number) => Promise<void>;
  emit: (e: HostEvent) => void;
  /** The demo host's General settings (launch at login, update channel). */
  settings: () => SettingsValues;
  setSettings: (patch: Partial<SettingsValues>) => void;
  signedIn: () => boolean;
  /** Time a change takes (ms). */
  stepMs: number;
  /** Settings, Experiments at start (the Volume demo starts with Cua Volume on). */
  experiments?: Experiments;
  /** The machine the demo runs on (a Mac unless the shell says). */
  platform?: DemoPlatform;
  /** A machine joined with a pairing code: the host lists it from now on. */
  addMachine?: (machine: MachineRow) => void;
  /** This device is not on the relay until Enroll… signs in again. */
  unenrolled?: boolean;
  /** Where the updater is at start (idle: up to date). */
  update?: Partial<UpdateState>;
}

/** How many times "Add a machine" reads the codes before the demo's other
 * computer runs the command (about ten seconds at its three-second poll). */
export const DEMO_PAIRING_POLLS = 3;

/** The computer the demo adds with a pairing code. */
export const DEMO_PAIRED_MACHINE: MachineRow = {
  id: "m_pair_demo",
  name: "Office PC",
  via: "relay",
  online: true,
  os: "linux",
  model: "Ubuntu 24.04",
  limits: [],
};

type Ops = SettingsOperations & NotificationsOperations;
type Handlers = { [K in keyof Ops]: (args: Ops[K]["args"]) => Promise<Ops[K]["result"]> | Ops[K]["result"] };

const DAY = 86_400;
const GIB = 1024 ** 3;

/** This Mac, enrolled. */
const thisDemoDevice = (now: number): DeviceInput => ({ id: "dev_mac", name: "MacBook Pro", state: "enrolled", current: true, platform: "macos", enrolledUntil: now + 24 * DAY, lastSeen: now - 60 });

/** The account's devices: this Mac (unless not enrolled yet), a Linux desktop, one waiting, one due. */
export function demoDevices(nowMs: number, unenrolled = false): DevicesInput {
  const now = Math.floor(nowMs / 1000);
  return {
    localDeviceId: "dev_mac",
    devices: [
      ...(unenrolled ? [] : [thisDemoDevice(now)]),
      { id: "dev_studio", name: "Studio", state: "enrolled", platform: "linux", enrolledUntil: now + 12 * DAY, lastSeen: now - 3 * 3600 },
      { id: "dev_work", name: "Work laptop", state: "pending", platform: "windows" },
      { id: "dev_old", name: "Old laptop", state: "expired", platform: "linux", enrolledUntil: now - 2 * DAY, lastSeen: now - 33 * DAY },
    ],
    audit: [
      { ts: now - 5 * 3600, kind: "machine_access", device: "dev_studio", machine: "m1", detail: "owner" },
      { ts: now - 2 * 3600, kind: "shared_access", machine: "m1", subject: "bob@example.com" },
      { ts: now - 1800, kind: "device_registered", device: "dev_work" },
      { ts: now - 600, kind: "machine_access", device: "dev_mac", machine: "m2", detail: "owner" },
    ],
    enforceAfter: now - 30 * DAY,
    machineNames: { m1: "studio-mac", m2: "mac-mini" },
    machines: [
      { id: "m1", name: "studio-mac", confirmed: true },
      { id: "m2", name: "mac-mini", confirmed: true },
    ],
  };
}

/** The daemon's feed: a finished turn, a message, an approval already handled. */
export function demoNotifications(nowMs: number): NotificationInput[] {
  return [
    { id: "n-ada-12", atMs: nowMs - 3 * 60_000, agent: "ada", kind: "turn_ended", title: "ada", body: "Your research is ready.\nThree sources, one open question.", read: false },
    { id: "n-hermes-4", atMs: nowMs - 41 * 60_000, agent: "hermes", kind: "message", title: "hermes", body: "The nightly build passed on all three Spaces.", read: false },
    { id: "n-codex-9", atMs: nowMs - 3 * 3_600_000, agent: "codex", kind: "approval", title: "codex asks to sign in", body: "Approve or deny it in Cua.", read: true },
    { id: "n-claw-2", atMs: nowMs - 26 * 3_600_000, agent: "openclaw", kind: "error", title: "openclaw ran into a problem", body: "The Space stopped while a turn was running.", read: true },
  ];
}

/**
 * When the demo feed was written: kept for the browser session, so a reload
 * doesn't make the same entries look new (a real feed keeps its times).
 */
function feedEpoch(nowMs: number): number {
  const key = "cua-spaces:demo-feed-epoch";
  try {
    const kept = Number(globalThis.sessionStorage?.getItem(key));
    if (kept > 0) return kept;
    globalThis.sessionStorage?.setItem(key, String(nowMs));
  } catch {
    // No session storage (tests): the clock is enough.
  }
  return nowMs;
}

function timeText(ms: number): string {
  return new Date(ms).toLocaleString("en-US", { month: "numeric", day: "numeric", year: "2-digit", hour: "numeric", minute: "2-digit" });
}

export function createDemoSettings(ctx: DemoSettingsContext): Handlers {
  const { now, wait, emit, stepMs } = ctx;
  let lastCheck: number | null = now() - 2 * 3_600_000;
  let checking = false;
  let autoCheck = true;
  let autoInstall = false;
  let experiments: Experiments = { ...NO_EXPERIMENTS, ...ctx.experiments };
  let devices = demoDevices(now(), ctx.unenrolled);
  // One code made earlier (`cua host pair`) that nobody used yet; only the
  // codes made here join a computer.
  const t0 = Math.floor(now() / 1000);
  let pairing: { code: PairingCode; polls: number; joins: boolean }[] = [
    {
      code: { id: "pc_demo0", code: null, url: null, installSh: null, installPowershell: null, state: "active", team: null, createdAt: t0 - 600, expiresAt: t0 + 1200, usedAt: null, machine: null },
      polls: 0,
      joins: false,
    },
  ];
  let pairingSeq = 0;
  let feed = demoNotifications(feedEpoch(now()));
  const platform = ctx.platform ?? MAC_DEMO_PLATFORM;
  const machine = demoMachine(platform);
  const storage: StorageInput = {
    os: platform.os,
    home: machine.home,
    storage: { backend: "fs", fs_path: machine.dataPath, s3: null, has_keys: false },
    mount: machine.mount(true),
    cache: { size_bytes: Math.round(1.2 * GIB), capacity_bytes: 10 * GIB },
  };

  let update: UpdateState = {
    status: "idle",
    currentVersion: "0.6.0",
    version: null,
    percent: null,
    notesUrl: null,
    message: null,
    ...ctx.update,
  };

  const about = (): AboutInput => ({
    platform: platform.os,
    version: "0.6.0",
    build: "0.6.0.142",
    os: machine.osText,
    updater: true,
    autoCheck,
    autoInstall,
    channel: ctx.settings().updateChannel ?? "stable",
    lastCheck: lastCheck === null ? null : timeText(lastCheck),
    checking,
  });
  const loginItem = (): LoginItemReport => ({
    status: ctx.settings().launchAtLogin ? "enabled" : "notRegistered",
    providesSpaces: false,
    runsAgents: true,
  });
  const device = (id: string) => {
    const d = devices.devices?.find((x) => x.id === id);
    if (!d) throw new HostError(`no device ${id}`, "not_found");
    return d;
  };
  const audit = (kind: string, subject: string) => {
    devices = { ...devices, audit: [...(devices.audit ?? []), { ts: Math.floor(now() / 1000), kind, device: "dev_mac", subject }] };
  };
  const patchDevice = (id: string, patch: Partial<NonNullable<DevicesInput["devices"]>[number]>) => {
    devices = { ...devices, devices: devices.devices?.map((d) => (d.id === id ? { ...d, ...patch } : d)) };
  };
  const ok = (applied: boolean): DriveCheckInput => ({ ok: true, reachable: true, authorized: true, versioning: true, detail: null, applied });

  return {
    "about.get": () => about(),
    "about.set": ({ autoCheck: c, autoInstall: i, channel }) => {
      if (c !== undefined) autoCheck = c;
      if (i !== undefined) autoInstall = i;
      if (channel) ctx.setSettings({ updateChannel: channel });
      return about();
    },
    "about.checkNow": () => {
      checking = true;
      void wait(stepMs * 2).then(() => {
        checking = false;
        lastCheck = now();
        emit({ type: "settings.changed" });
      });
      return about();
    },

    "update.get": () => ({ ...update }),
    "update.install": () => {
      // Nothing downloaded: nothing to install, and the state says so.
      if (update.status !== "downloaded") return { ...update };
      // The real host relaunches into the new version.
      update = { ...update, status: "idle", currentVersion: update.version ?? update.currentVersion, version: null, notesUrl: null };
      emit({ type: "update.changed" });
      return { ...update };
    },

    "experiments.get": () => ({ ...experiments }),
    "experiments.set": ({ experiments: next }) => {
      experiments = { ...next };
      emit({ type: "settings.changed" });
      return { ...experiments };
    },

    "loginItem.get": () => loginItem(),
    "loginItem.set": async ({ on }) => {
      await wait(stepMs);
      ctx.setSettings({ launchAtLogin: on });
      return loginItem();
    },
    "loginItem.openSettings": () => null,

    "devices.get": () => {
      if (!ctx.signedIn()) throw new HostError("Sign in to Cua to see your devices", "signed_out");
      return structuredClone(devices);
    },
    // Signing in again enrolls this device at once.
    "devices.enroll": () => {
      if (!devices.devices?.some((d) => d.id === "dev_mac")) {
        devices = { ...devices, devices: [thisDemoDevice(Math.floor(now() / 1000)), ...(devices.devices ?? [])] };
        audit("device_enrolled", "dev_mac");
      }
      return { enrolled: true, code: null };
    },
    "devices.checkEnrolled": () => true,
    "devices.approve": async ({ code, deviceId }) => {
      await wait(stepMs);
      const target = deviceId ? device(deviceId) : devices.devices?.find((d) => d.state === "pending");
      if (!target || (code && code !== "K7QX-M2RP")) {
        throw new HostError(`not found: relay: no device is waiting with the code ${code} (codes expire after 10 minutes)`, "not_found");
      }
      patchDevice(target.id, { state: "enrolled", enrolledUntil: Math.floor(now() / 1000) + 30 * DAY });
      audit("device_enrolled", target.id);
      return null;
    },
    "devices.rename": ({ id, name }) => {
      device(id);
      patchDevice(id, { name });
      audit("device_renamed", id);
      return null;
    },
    "devices.revoke": async ({ id }) => {
      device(id);
      await wait(stepMs / 2);
      patchDevice(id, { state: "revoked" });
      audit("device_revoked", id);
      return null;
    },
    "devices.confirmMachine": ({ id }) => {
      devices = { ...devices, machines: devices.machines?.map((m) => (m.id === id ? { ...m, confirmed: true } : m)) };
      return null;
    },

    // Works like the relay: one code per machine, 30 minutes, shown once.
    // The demo's other computer runs the command after a few reads.
    "devices.createPairingCode": async ({ team }) => {
      if (!ctx.signedIn()) throw new HostError("Sign in to Cua to add a machine", "signed_out");
      await wait(stepMs / 2);
      const t = Math.floor(now() / 1000);
      pairingSeq += 1;
      const code = `K7QM-4XPB-${["2RHD", "9TCW", "5JNF", "8LGV"][(pairingSeq - 1) % 4]}`;
      const made: PairingCode = {
        id: `pc_demo${pairingSeq}`,
        code,
        url: `https://cua.ai/add/${code}`,
        installSh: `curl -fsSL https://cua.ai/install.sh | sh -s -- --pair ${code}`,
        installPowershell: `& ([scriptblock]::Create((irm https://cua.ai/install.ps1))) -Pair ${code}`,
        state: "active",
        team: team ?? null,
        createdAt: t,
        expiresAt: t + 30 * 60,
        usedAt: null,
        machine: null,
      };
      pairing = [{ code: made, polls: 0, joins: true }, ...pairing];
      return structuredClone(made);
    },
    "devices.pairingCodes": () => {
      const t = Math.floor(now() / 1000);
      pairing = pairing.map((p) => {
        const c = p.code;
        if (c.state === "active" && t >= c.expiresAt) return { ...p, code: { ...c, state: "expired" } };
        if (c.state !== "active" || !p.joins) return p;
        const polls = p.polls + 1;
        if (polls < DEMO_PAIRING_POLLS) return { ...p, polls };
        ctx.addMachine?.({ ...DEMO_PAIRED_MACHINE, lastSeen: t });
        return { ...p, polls, code: { ...c, state: "used", usedAt: t, machine: DEMO_PAIRED_MACHINE.id } };
      });
      // Listed without the codes, as the relay lists them.
      return pairing.map(({ code: c }) => ({ ...c, code: null, url: null, installSh: null, installPowershell: null }));
    },
    // The demo account owns a team on the Teams plan, and is a member of another.
    "teams.list": () => ({
      teams: [
        { id: "0f1e2d3c-0000-4000-8000-0000000000a1", alias: "acme", name: "Acme", role: "owner", plan: "teams" },
        { id: "0f1e2d3c-0000-4000-8000-0000000000b2", alias: "side", name: "", role: "member", plan: "teams" },
      ],
      currentTeamId: null,
    }),
    "devices.revokePairingCode": ({ id }) => {
      const p = pairing.find((x) => x.code.id === id);
      if (!p) throw new HostError(`no pairing code ${id}`, "not_found");
      if (p.code.state !== "active") throw new HostError(`This code was already ${p.code.state}.`, "conflict");
      pairing = pairing.map((x) => (x === p ? { ...x, code: { ...x.code, state: "revoked" } } : x));
      return null;
    },

    "storage.get": () => structuredClone(storage),
    "storage.run": async ({ request }) => {
      await wait(stepMs);
      switch (request.kind) {
        case "test":
          return request.update.s3?.bucket ? ok(false) : { ...ok(false), ok: false, reachable: false };
        case "save":
        case "adopt":
          storage.storage = {
            backend: request.update.backend,
            fs_path: storage.storage?.fs_path ?? "",
            s3: request.update.s3,
            has_keys: Boolean(request.update.access_key_id) || Boolean(storage.storage?.has_keys),
          };
          return ok(true);
        case "mount":
          storage.mount = machine.mount(true);
          return null;
        case "unmount":
          storage.mount = machine.mount(false);
          return null;
        case "set-cache":
          storage.cache = { ...storage.cache!, capacity_bytes: request.capacity_bytes };
          return null;
        case "clear-cache":
          storage.cache = { ...storage.cache!, size_bytes: 0 };
          return null;
        case "reveal":
        case "open-url":
          return null;
      }
    },

    "notifications.list": () => feed.map((n) => ({ ...n })),
    "notifications.markAllRead": () => {
      feed = feed.map((n) => ({ ...n, read: true }));
      return null;
    },
    "notifications.markRead": ({ ids }) => {
      feed = feed.map((n) => (ids.includes(n.id) ? { ...n, read: true } : n));
      return null;
    },
  };
}
