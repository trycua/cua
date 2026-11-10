// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Cua Spaces app core on the phone. `call(method, args)` is the same JSON
 * dispatcher the web app loads as wasm and the SwiftUI app reaches through
 * UniFFI (libs/cua/crates/cua-spaces-app-core). On iOS it is a Rust static
 * library behind the `cua-core` Expo module (Hermes has no WebAssembly).
 *
 * Where the library is not linked (Android and web until it is built there,
 * Jest unless a test installs a core), `available` is false and callers show
 * the screen's plain fallback; nothing here answers in the core's place.
 */

import { native } from "../../modules/cua-core/src";

export interface CoreClient {
  /** The Rust core answers. */
  readonly available: boolean;
  /** Calls `method`; throws the core's error (or when unavailable). */
  call<T>(method: string, args?: Record<string, unknown>): T;
  /** Like `call`, but null when the core is unavailable or refuses. */
  tryCall<T>(method: string, args?: Record<string, unknown>): T | null;
}

export class CoreUnavailableError extends Error {
  constructor(method: string) {
    super(`The app core is not linked in this build (${method}).`);
    this.name = "CoreUnavailableError";
  }
}

export function makeCore(invoke: ((method: string, args: string) => string) | null): CoreClient {
  return {
    available: invoke !== null,
    call<T>(method: string, args: Record<string, unknown> = {}): T {
      if (!invoke) throw new CoreUnavailableError(method);
      return JSON.parse(invoke(method, JSON.stringify(args))) as T;
    },
    tryCall<T>(method: string, args: Record<string, unknown> = {}): T | null {
      if (!invoke) return null;
      try {
        return JSON.parse(invoke(method, JSON.stringify(args))) as T;
      } catch {
        return null;
      }
    },
  };
}

let current: CoreClient = makeCore(native?.available ? (m, a) => native!.call(m, a) : null);

/** The core the app uses. */
export const core: CoreClient = {
  get available() {
    return current.available;
  },
  call: (m, a) => current.call(m, a),
  tryCall: (m, a) => current.tryCall(m, a),
};

/** Tests (and nothing else) install another core. */
export function setCoreForTests(c: CoreClient): void {
  current = c;
}
