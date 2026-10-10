// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// This machine as a client of the account on the relay (the SwiftUI app's
// DevicesModel.swift): the relay's devices snapshot, read again when stale,
// and what the Machines page shows from it (the core's `appDevicesView`,
// who the relay sees connected, each device's state, and why this machine
// cannot open the account's machines), enrolling this machine, approving
// another device after the owner check (`confirmPresence`), the rows'
// buttons, and announcing a device that asks to join (a system
// notification, once per launch).
import { execFileSync } from "node:child_process";
import * as os from "node:os";
import type { Native } from "../native/load";
import type { AppApprovalPrompt, AppDevicesInput, AppDevicesView, AppMachineAccessNotice, DevicesLike, DevicesSnapshot, RelayDevice } from "../native/generated/index";
import { words } from "./errors";

/** What this machine is called on the relay: macOS's computer name ("Dana's MacBook Pro"), else the hostname. */
export function computerName(platform: NodeJS.Platform = process.platform): string {
  if (platform === "darwin") {
    try {
      const name = execFileSync("scutil", ["--get", "ComputerName"], { encoding: "utf8", timeout: 2000 }).trim();
      if (name) return name;
    } catch {
      // Falls back to the hostname.
    }
  }
  return os.hostname();
}

export class DevicesModel {
  /** The relay's answer, once read. */
  snapshot: DevicesSnapshot | null = null;
  /** When `snapshot` was read (ms). */
  refreshedAt: number | null = null;
  /** The code this machine shows while it waits for approval. */
  pendingCode: string | null = null;
  error: string | null = null;
  /** Signed in to Cua (no account, no devices). */
  signedIn = false;
  clock: () => number = () => Date.now();
  /** Asks the person at this machine to confirm before approving a device (Touch ID or the login password on macOS); throws when they did not. */
  presence: ((reason: string) => Promise<void>) | null = null;
  /** Shows a system notification for a device asking to join. */
  notify: ((prompt: AppApprovalPrompt) => void) | null = null;
  /** Approvals already announced (a device asks once per launch). */
  readonly announced = new Set<string>();
  /** Re-verifications put off with Not Now. */
  readonly dismissed = new Set<string>();
  /** Called after each read and enrollment step, so the page hears what moved (the Swift
   * model is observed; the app model wires this to `machines`, and the bridge tells the
   * page only when what it shows differs). An open detail follows an approval this way. */
  onChange: (() => void) | null = null;
  private refreshing: Promise<void> | null = null;

  constructor(
    private readonly native: Native,
    /** The SDK's `Devices` for the signed-in session (null: not made yet). */
    public devices: () => DevicesLike | null,
  ) {}

  get input(): AppDevicesInput {
    if (this.snapshot) return this.native.appDevicesInput(this.snapshot, this.pendingCode ?? undefined);
    return { devices: [], audit: [], localDeviceId: undefined, pendingCode: this.pendingCode ?? undefined, enforceAfter: undefined, machineNames: new Map(), machines: [] };
  }

  /** The Devices page. */
  get view(): AppDevicesView {
    return this.native.appDevicesView(this.input, BigInt(Math.floor(this.clock() / 1000)));
  }

  /** Signed in, read, and this machine cannot open the account's machines: why, and its one action. */
  get accessNotice(): AppMachineAccessNotice | undefined {
    if (!this.signedIn || !this.snapshot) return undefined;
    return this.native.appMachineAccessNotice(this.view.thisDevice.kind, this.pendingCode ?? undefined);
  }

  /** Whether the relay saw each of the account's machines connected, by machine id. */
  get relayOnline(): Record<string, boolean> {
    const out: Record<string, boolean> = {};
    for (const m of this.snapshot?.machines ?? []) if (!(m.id in out)) out[m.id] = m.online;
    return out;
  }

  /** Each device's state (`enrolled`, `pending`, `expired`, `revoked`), by id. */
  get deviceStates(): Record<string, string> {
    const out: Record<string, string> = {};
    for (const d of this.snapshot?.devices ?? []) if (!(d.id in out)) out[d.id] = d.state;
    return out;
  }

  /** Reads the relay again when the last answer is older than `maxAgeMs`. */
  async refreshIfStale(maxAgeMs = 30_000): Promise<void> {
    if (this.refreshedAt !== null && this.clock() - this.refreshedAt < maxAgeMs) return;
    await this.refresh();
  }

  /** Reads the relay; announces devices newly asking for approval. One read at a time. */
  refresh(): Promise<void> {
    this.refreshing ??= this.read().finally(() => {
      this.refreshing = null;
      this.onChange?.();
    });
    return this.refreshing;
  }

  private async read(): Promise<void> {
    const devices = this.signedIn ? this.devices() : null;
    if (!devices) {
      this.snapshot = null;
      return;
    }
    try {
      this.snapshot = await devices.snapshot(50);
      this.refreshedAt = this.clock();
      this.error = null;
    } catch (error) {
      this.error = words(error);
      return;
    }
    const view = this.view;
    if (view.enrolled) this.pendingCode = null;
    // Announced once a notification can go out (the app wires it at start).
    for (const prompt of this.notify ? view.approvals : []) {
      if (this.announced.has(prompt.deviceId)) continue;
      this.announced.add(prompt.deviceId);
      if (prompt.expired && this.dismissed.has(prompt.deviceId)) continue;
      this.notify?.(prompt);
    }
  }

  private relay(): DevicesLike {
    const devices = this.signedIn ? this.devices() : null;
    if (!devices) throw new DevicesUnavailable();
    return devices;
  }

  /** Registers this machine: enrolled at once, or the code another device approves. */
  async enroll(): Promise<{ enrolled: boolean; code: string | null }> {
    const r = await this.relay().enroll();
    this.pendingCode = r.enrolled ? null : (r.code ?? null);
    this.onChange?.();
    return { enrolled: r.enrolled, code: r.code ?? null };
  }

  /** Whether an enrolled device approved this one yet. */
  async checkEnrolled(): Promise<boolean> {
    const ok = await this.relay().checkEnrolled();
    if (ok) this.pendingCode = null;
    this.onChange?.();
    return ok;
  }

  /** Approve another device (by its code, or its id): the owner check first, then the relay; "approved" only once the relay says it is enrolled. */
  async approve(code: string | null, deviceId: string | null): Promise<void> {
    const relay = this.relay();
    const name = (deviceId && this.input.devices.find((d) => d.id === deviceId)?.name) || "this device";
    // Never without the owner check.
    if (!this.presence) throw new Error("Approving needs the owner check, which this build does not have");
    await this.presence(`approve \u201c${name}\u201d for your Cua account`);
    requireEnrolled(await relay.approve(code ?? undefined, deviceId ?? undefined));
    await this.refresh();
  }

  async rename(id: string, name: string): Promise<void> {
    const clean = this.native.appDevicesCleanName(name);
    if (!clean) throw new RangeError("name");
    await this.relay().rename(id, clean);
    await this.refresh();
  }

  async revoke(id: string): Promise<void> {
    await this.relay().revoke(id);
    await this.refresh();
  }

  /** Vouches for a machine that registered without an enrolled device's proof. */
  async confirmMachine(id: string): Promise<void> {
    await this.relay().confirmMachine(id);
    await this.refresh();
  }
}

/** Devices need a signed-in Cua account. */
export class DevicesUnavailable extends Error {
  constructor() {
    super("Devices need a signed-in Cua account");
    this.name = "DevicesUnavailable";
  }
}

/** Only say "approved" when the relay says the device is enrolled: anything else would leave the other device waiting while this one reads success. */
export function requireEnrolled(device: RelayDevice): void {
  if (device.state !== "enrolled") {
    throw new Error(`The relay did not enroll ${device.name} (${device.id}): it is ${device.state}. Ask that device for a new code and try again.`);
  }
}
