// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * What the phone keeps on disk between launches: the outbox, cached threads and the last Spaces list.
 *
 * Every value is sealed (AES-256-GCM, `core/src/store.rs`) with a key that lives in the Keychain on this phone only,
 * bound to its slot name, so a message never sits on disk as plain text and a value copied to another slot does not
 * open. JavaScript holds only the sealed text. Where there is no native sealer (Android until the core is built there,
 * and Jest unless a test installs one) the store is `available: false`: reads find nothing, writes are dropped, and the
 * app says nothing is kept rather than keeping it in the clear.
 */

import AsyncStorage from "@react-native-async-storage/async-storage";

import { native } from "../../modules/cua-core/src";

const PREFIX = "cua.sealed.v1:";

export interface Sealer {
  seal(plaintext: string, aad: string): string;
  open(sealed: string, aad: string): string;
  reset?(): void;
}

export interface KeyValue {
  getItem(key: string): Promise<string | null>;
  setItem(key: string, value: string): Promise<void>;
  removeItem(key: string): Promise<void>;
  getAllKeys(): Promise<readonly string[]>;
}

export interface SealedStore {
  /** Values are really kept (a sealer is linked). */
  readonly available: boolean;
  get(slot: string): Promise<string | null>;
  set(slot: string, value: string): Promise<void>;
  remove(slot: string): Promise<void>;
  /** Everything this store wrote, and the key: sign-out. */
  clear(): Promise<void>;
}

/** The native sealer, or null where it is not linked. */
export function nativeSealer(): Sealer | null {
  if (!native?.available || typeof native.storeSeal !== "function" || typeof native.storeOpen !== "function") return null;
  return { seal: (p, a) => native!.storeSeal!(p, a), open: (s, a) => native!.storeOpen!(s, a), reset: () => native!.storeReset?.() };
}

export function createSealedStore(opts: { sealer?: Sealer | null; storage?: KeyValue; namespace?: string } = {}): SealedStore {
  const sealer = opts.sealer === undefined ? nativeSealer() : opts.sealer;
  const kv: KeyValue = opts.storage ?? (AsyncStorage as unknown as KeyValue);
  const ns = `${PREFIX}${opts.namespace ?? "live"}:`;
  return {
    available: sealer !== null,
    async get(slot) {
      if (!sealer) return null;
      try {
        const sealed = await kv.getItem(ns + slot);
        return sealed ? sealer.open(sealed, slot) : null;
      } catch {
        // A value that does not open (another key, a changed file) is gone, not an error for the person.
        await kv.removeItem(ns + slot).catch(() => {});
        return null;
      }
    },
    async set(slot, value) {
      if (!sealer) return;
      try {
        await kv.setItem(ns + slot, sealer.seal(value, slot));
      } catch {
        /* a full disk or a locked keychain: the app carries on without a saved copy */
      }
    },
    async remove(slot) {
      await kv.removeItem(ns + slot).catch(() => {});
    },
    async clear() {
      try {
        const keys = (await kv.getAllKeys()).filter((k) => k.startsWith(PREFIX));
        await Promise.all(keys.map((k) => kv.removeItem(k)));
      } catch {
        /* nothing to clear */
      }
      try {
        sealer?.reset?.();
      } catch {
        /* the key is already gone */
      }
    },
  };
}
