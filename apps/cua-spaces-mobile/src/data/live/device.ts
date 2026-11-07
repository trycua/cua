// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * This phone as a client device of the cua.ai account on a relay (`cua-host::device`). The P-256 key, its id and the
 * signatures are the Rust core's (`core/src/device.rs`, checked against `cua_host::device` by
 * `core/tests/device_known_answers.rs`) and the key stays in the Keychain on the native side: JavaScript only ever
 * holds the public key, the id and signatures. The message formats the relay checks are in the shared file
 * `core/fixtures/device-messages.json`, which the Rust test reads too.
 */

import messages from "../../../core/fixtures/device-messages.json";
import { native } from "../../../modules/cua-core/src";

export interface DeviceSigner {
  /** The id (`dev_` and 24 hex characters) and the public key (uncompressed point, base64url). */
  readonly id: string;
  readonly publicKey: string;
  /** A fixed-size ECDSA signature of the SHA-256 of `message`, base64url. */
  sign(message: string): string;
}

const fill = (t: string, vars: Record<string, string | number>) => Object.entries(vars).reduce((s, [k, v]) => s.replaceAll(`{${k}}`, String(v)), t);

export const registerMessage = (device: string, ts: number) => fill(messages.register, { device, ts });
export const sessionMessage = (device: string, ts: number) => fill(messages.session, { device, ts });

/** The phone's signer, or null where the core is not linked (Android until it is built there): the app then says so. */
export function nativeSigner(): DeviceSigner | null {
  if (!native?.available || typeof native.deviceKey !== "function") return null;
  const { id, publicKey } = native.deviceKey();
  return { id, publicKey, sign: (m) => native!.deviceSign(m) };
}
