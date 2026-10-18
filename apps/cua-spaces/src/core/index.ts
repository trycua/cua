// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Cua Spaces app core (libs/cua/crates/cua-spaces-app-core) in the
 * webview, as wasm (apps/cua-spaces/core-wasm, built into ./wasm by
 * scripts/build-core-wasm.mjs). The same core the Tauri shell links and the
 * SwiftUI app binds, so every shell makes the same decisions.
 *
 * `initCore()` runs once before the first render (main.tsx); tests load it
 * synchronously (src/test/setup.ts). `core()` is synchronous after that.
 */
import init, { call, initSync } from "./wasm/core.js";

let ready = false;

/** Loads the wasm (browser and Tauri webview). */
export async function initCore(): Promise<void> {
  if (ready) return;
  await init();
  ready = true;
}

/** Loads the wasm from bytes (tests, Node). */
export function initCoreSync(bytes: BufferSource): void {
  if (ready) return;
  initSync({ module: bytes });
  ready = true;
}

/** Calls a core method with named arguments; returns its JSON result. */
export function core<T>(method: string, args: Record<string, unknown> = {}): T {
  if (!ready) throw new Error("the app core is not loaded: call initCore() first");
  return JSON.parse(call(method, JSON.stringify(args))) as T;
}
