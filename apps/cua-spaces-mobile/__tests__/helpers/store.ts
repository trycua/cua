// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createSealedStore, type KeyValue, type Sealer, type SealedStore } from "../../src/store/sealed";

/** An in-memory stand-in for AsyncStorage (what the sealed store writes to). */
export function memoryKv(): KeyValue & { data: Map<string, string> } {
  const data = new Map<string, string>();
  return {
    data,
    getItem: async (k) => data.get(k) ?? null,
    setItem: async (k, v) => void data.set(k, v),
    removeItem: async (k) => void data.delete(k),
    getAllKeys: async () => [...data.keys()],
  };
}

/**
 * A sealer for tests: it binds a value to its slot like the real one does (AES-256-GCM in `core/src/store.rs`, whose
 * known answers `cargo test` checks), but it is not encryption. Tests that need to see "not plain text" read the real
 * ciphertext on the Simulator instead.
 */
export const fakeSealer: Sealer & { resets: number } = {
  resets: 0,
  seal: (p, a) => `v1.${Buffer.from(JSON.stringify({ a, p })).toString("base64url")}`,
  open: (s, a) => {
    const o = JSON.parse(Buffer.from(s.slice(3), "base64url").toString()) as { a: string; p: string };
    if (o.a !== a) throw new Error("wrong slot");
    return o.p;
  },
  reset() {
    fakeSealer.resets += 1;
  },
};

export function testStore(kv: KeyValue = memoryKv(), namespace = "test"): SealedStore {
  return createSealedStore({ sealer: fakeSealer, storage: kv, namespace });
}
