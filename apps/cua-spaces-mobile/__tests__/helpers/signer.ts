// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { p256 } from "@noble/curves/nist.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { bytesToHex, utf8ToBytes } from "@noble/hashes/utils.js";

import type { DeviceSigner } from "../../src/data/live/device";

/**
 * A test double for the native signer: the same scheme as core/src/device.rs (P-256, SHA-256, fixed-size signature,
 * base64url; id from the public key). It only lets the relay client's tests verify what it sends; the real signer is
 * Rust, checked against cua_host::device in core/tests/device_known_answers.rs.
 */
const B64 = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
export function b64url(bytes: Uint8Array): string {
  let out = "";
  for (let i = 0; i < bytes.length; i += 3) {
    const n = (bytes[i]! << 16) | ((bytes[i + 1] ?? 0) << 8) | (bytes[i + 2] ?? 0);
    out += B64[(n >> 18) & 63]! + B64[(n >> 12) & 63]!;
    if (i + 1 < bytes.length) out += B64[(n >> 6) & 63]!;
    if (i + 2 < bytes.length) out += B64[n & 63]!;
  }
  return out;
}
export function fromB64url(s: string): Uint8Array {
  const idx = new Map([...B64].map((c, i) => [c, i]));
  const out: number[] = [];
  let acc = 0;
  let bits = 0;
  for (const ch of s) {
    acc = (acc << 6) | (idx.get(ch) ?? 0);
    bits += 6;
    if (bits >= 8) {
      bits -= 8;
      out.push((acc >> bits) & 255);
    }
  }
  return Uint8Array.from(out);
}

export function testSigner(): DeviceSigner {
  const secret = p256.utils.randomSecretKey();
  const pub = p256.getPublicKey(secret, false);
  return {
    id: `dev_${bytesToHex(sha256(pub)).slice(0, 24)}`,
    publicKey: b64url(pub),
    sign: (m) => b64url(p256.sign(sha256(utf8ToBytes(m)), secret, { prehash: false })),
  };
}

export const verify = (publicKey: string, message: string, signature: string) =>
  p256.verify(fromB64url(signature), sha256(utf8ToBytes(message)), fromB64url(publicKey), { prehash: false });
