// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * This phone's enrollment on the relay (`cua-host::device::DeviceAuth`): a fresh interactive sign-in enrolls the key
 * right away on a relay that takes the sign-in as the second factor, else it shows a one-time code that an enrolled
 * machine approves. The key is made and kept natively (`nativeSigner`).
 */

import Constants from "expo-constants";
import { Platform } from "react-native";

import { nativeSigner, type DeviceSigner } from "./device";
import { isNotEnrolled, type WaitingApproval } from "./reach";
import type { Enrollment, RelayClient } from "./relay";

const SESSION_MARGIN = 60;
const RETRY_SECONDS = 3;

/**
 * The name this phone is enrolled under: the phone's own ("Dillon's iPhone"), as other screens will list it. Never a
 * first-person word like "This phone": that only makes sense on the phone itself (the phone's own screens add a badge).
 */
export function realDeviceName(raw: string | null | undefined = Constants.deviceName, os: string = Platform.OS, isPad: boolean = Platform.OS === "ios" && (Platform as { isPad?: boolean }).isPad === true): string {
  const name = (raw ?? "").replace(/\s+/g, " ").trim();
  if (name && !/^this (phone|device|ipad|iphone)$/i.test(name)) return name.slice(0, 64);
  return os === "ios" ? (isPad ? "iPad" : "iPhone") : "Android phone";
}

/** What this phone states it can host for the pool: nothing today. Versioned and additive. */
export const PHONE_CAPABILITIES = { v: 1, can_host_spaces: false, images: [] as string[], kinds: [] as string[], desktop: false };

export type EnrollState = { state: "enrolled" } | { state: "pending"; code: string | null; expiresAt: number | null } | { state: "refused"; message: string };

const NO_KEY = "This build cannot hold this phone's key yet (the app core is not linked on this platform), so the phone cannot be added.";

export class DeviceManager {
  private cached: { session: string; expires: number } | null = null;
  /** The relay refused a session because this phone is pending or not enrolled: the code it shows, and when it was asked. */
  private waiting: (WaitingApproval & { at: number }) | null = null;
  private code: string | null = null;

  constructor(
    private readonly relay: RelayClient,
    private readonly token: () => Promise<string>,
    private readonly signer: () => DeviceSigner | null = nativeSigner,
    private readonly name: string = realDeviceName(),
    private readonly now = () => Math.floor(Date.now() / 1000),
    private readonly platform: string = Platform.OS,
  ) {}

  /** This phone's relay device id (`dev_...`), when the key is available. */
  id(): string | null {
    return this.signer()?.id ?? null;
  }

  /** Set while the relay says this phone is waiting for approval (pending or not enrolled); Home shows it with the code. */
  approval(): WaitingApproval | null {
    return this.waiting ? { code: this.waiting.code } : null;
  }

  /**
   * A device session for relay calls; null when this phone has none yet (the relay then decides, as during its grace
   * period). A relay that says the phone is pending or not enrolled is remembered (`approval()`), never swallowed, and the
   * next call asks again (at most every few seconds), so pulling down on Home retries.
   */
  async session(): Promise<string | null> {
    if (this.cached && this.cached.expires > this.now() + SESSION_MARGIN) return this.cached.session;
    const key = this.signer();
    if (!key) return null;
    if (this.waiting && this.now() - this.waiting.at < RETRY_SECONDS) return null;
    try {
      const s = await this.relay.deviceSession(await this.token(), key, this.now());
      this.cached = { session: s.session, expires: s.expires_at };
      this.waiting = null;
      return s.session;
    } catch (err) {
      if (isNotEnrolled(err)) {
        const first = !this.waiting;
        this.waiting = { code: this.code, at: this.now() };
        // A phone that has no code yet (the app was restarted) asks the relay for one, so Home can show the command.
        if (first && !this.code) await this.refreshCode(key);
      }
      return null;
    }
  }

  private async refreshCode(key: DeviceSigner): Promise<void> {
    try {
      const name = !this.name.trim() || /^this (phone|device)$/i.test(this.name.trim()) ? realDeviceName() : this.name;
      const e = await this.relay.registerDevice(await this.token(), key, name, this.platform === "ios" ? "ios" : "android", this.now(), PHONE_CAPABILITIES);
      if (e.device.state !== "enrolled" && e.code) {
        this.code = e.code;
        if (this.waiting) this.waiting = { ...this.waiting, code: e.code };
      }
    } catch {
      /* the command is still shown, with a placeholder for the code */
    }
  }

  /** Registers this phone after an interactive sign-in. */
  async enroll(): Promise<EnrollState> {
    const key = this.signer();
    if (!key) return { state: "refused", message: NO_KEY };
    let e: Enrollment;
    try {
      const name = !this.name.trim() || /^this (phone|device)$/i.test(this.name.trim()) ? realDeviceName() : this.name;
      e = await this.relay.registerDevice(await this.token(), key, name, this.platform === "ios" ? "ios" : "android", this.now(), PHONE_CAPABILITIES);
    } catch (err) {
      return { state: "refused", message: err instanceof Error ? err.message : String(err) };
    }
    this.cached = null;
    if (e.device.state === "enrolled") return { state: "enrolled" };
    if (e.code) this.code = e.code;
    return { state: "pending", code: e.code ?? null, expiresAt: e.device.code_expires ?? null };
  }

  /** Whether the approval landed (asks the relay for a session). */
  async check(): Promise<EnrollState> {
    const key = this.signer();
    if (!key) return { state: "refused", message: NO_KEY };
    try {
      const s = await this.relay.deviceSession(await this.token(), key, this.now());
      this.cached = { session: s.session, expires: s.expires_at };
      this.waiting = null;
      return { state: "enrolled" };
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      return /pending|not enrolled|approve/i.test(message) ? { state: "pending", code: this.code, expiresAt: null } : { state: "refused", message };
    }
  }
}
