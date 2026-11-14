// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { existsSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { coreFromBytes, unavailableCore, type CoreClient, type CoreWasmModule } from "../core";

// import.meta.url is not a file: URL under jsdom; dirname is.
const dir = `${join(import.meta.dirname, "..", "core", "wasm")}/`;
export const wasmBuilt = existsSync(`${dir}core_bg.wasm`) && existsSync(`${dir}core.js`);

let cached: CoreClient | undefined;

/** The real app core (wasm, loaded from bytes), or `unavailable` when not built. */
export async function testCore(): Promise<CoreClient> {
  if (!wasmBuilt) return unavailableCore("not built");
  if (cached) return cached;
  const mod = (await import(/* @vite-ignore */ `${dir}core.js`)) as CoreWasmModule;
  cached = coreFromBytes(mod, readFileSync(`${dir}core_bg.wasm`));
  return cached;
}

export const noCore = unavailableCore("test: no core");

export const tick = (ms = 0) => new Promise((r) => setTimeout(r, ms));

/** Polls `f` until it stops throwing (or times out). */
export async function until<T>(f: () => T, timeoutMs = 2000): Promise<T> {
  const start = Date.now();
  for (;;) {
    try {
      return f();
    } catch (e) {
      if (Date.now() - start > timeoutMs) throw e;
      await tick(5);
    }
  }
}
