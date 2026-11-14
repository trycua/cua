// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { requireOptionalNativeModule } from "expo-modules-core";

interface Native {
  available: boolean;
  call(method: string, args: string): string;
  methods(): string;
  /** This phone's device key, made on first use and kept in the Keychain. */
  deviceKey(): { publicKey: string; id: string };
  deviceSign(message: string): string;
  flows(): string;
  /** The sealed store (AES-256-GCM, key in the Keychain): `aad` names the slot a value belongs to. */
  storeSeal?(plaintext: string, aad: string): string;
  storeOpen?(sealed: string, aad: string): string;
  /** Drops the store key (sign-out): what is left sealed on disk can never be opened. */
  storeReset?(): void;
}

/** The native module, or null where the Rust core is not linked (Android and web for now). */
export const native: Native | null = requireOptionalNativeModule<Native>("CuaCore");
