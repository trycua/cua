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
import type { Enrollment, RelayClient } from "./relay";

const SESSION_MARGIN = 60;

/**
 * The name this phone is enrolled under: the phone's own ("Dillon's iPhone"), as other screens will list it. Never a
 * first-person word like "This phone": that only makes sense on the phone itself (the phone's own screens add a badge).
 */
export function realDeviceName(
  raw: string | null | undefined = Constants.deviceName,
  os: string = Platform.OS,
  isPad: boolean = Platform.OS === "ios" && (Platform as { isPad?: boolean }).isPad === true,
): string {
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

  /** A device session for relay calls; null when this phone is not enrolled yet (the relay then decides, as during its grace period). */
  async session(): Promise<string | null> {
    if (this.cached && this.cached.expires > this.now() + SESSION_MARGIN) return this.cached.session;
    const key = this.signer();
    if (!key) return null;
    try {
      const s = await this.relay.deviceSession(await this.token(), key, this.now());
      this.cached = { session: s.session, expires: s.expires_at };
      return s.session;
    } catch {
      return null;
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
    return { state: "pending", code: e.code ?? null, expiresAt: e.device.code_expires ?? null };
  }

  /** Whether the approval landed (asks the relay for a session). */
  async check(): Promise<EnrollState> {
    const key = this.signer();
    if (!key) return { state: "refused", message: NO_KEY };
    try {
      const s = await this.relay.deviceSession(await this.token(), key, this.now());
      this.cached = { session: s.session, expires: s.expires_at };
      return { state: "enrolled" };
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      return /pending|not enrolled|approve/i.test(message) ? { state: "pending", code: null, expiresAt: null } : { state: "refused", message };
    }
  }
}
