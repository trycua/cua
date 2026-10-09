// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/// <reference types="vite/client" />

/**
 * The Cua Spaces app core (libs/cua/crates/cua-spaces-app-core) in the
 * page, as wasm. Same crate and shim as the Tauri app
 * (apps/cua-spaces/core-wasm); `scripts/build-core-wasm.mjs` writes the
 * wasm-bindgen output to ./wasm (gitignored).
 *
 * Every host loads it the same way: it is plain wasm, not a host feature.
 * When ./wasm was not built, or fails to load, the core is `unavailable` and
 * the bridge falls back to its TypeScript stand-ins (see `../derive.ts`).
 */

/** The wasm-bindgen module (`core.js`, `--target web`). */
export interface CoreWasmModule {
  default: (input?: unknown) => Promise<unknown>;
  initSync: (options: { module: BufferSource | WebAssembly.Module }) => unknown;
  call: (method: string, args: string) => string;
  methods: () => string;
  /** The parity goldens: `[{name, flow, golden}]` as JSON. */
  flows?: () => string;
  /** Replays a flow through a JS host `(method, argsJson) => resultJson`. */
  runFlow?: (name: string, flow: string, host: (method: string, args: string) => string) => string;
}

/** One parity golden (`cua-spaces-app-core/parity/*.json`). */
export interface ParityFlow {
  name: string;
  flow: string;
  golden: string;
}

/** The core's parity harness. */
export interface CoreParity {
  flows(): ParityFlow[];
  /** Replays `flow` with `host` answering each step; returns the transcript. */
  run(name: string, flow: string, host: (method: string, args: Record<string, unknown>) => unknown): unknown;
}

export type CoreStatus = "loading" | "ready" | "unavailable";

export interface CoreClient {
  readonly status: CoreStatus;
  /** Why it is unavailable. */
  readonly reason?: string;
  /** Every method the core answers (empty unless ready). */
  readonly methods: readonly string[];
  /** Calls a core method; throws when unavailable or when the core refuses. */
  call<T>(method: string, args?: Record<string, unknown>): T;
  /** `call`, or `undefined` when the core is unavailable or lacks `method`.
   * Errors from the core itself still throw. */
  tryCall<T>(method: string, args?: Record<string, unknown>): T | undefined;
  /** The parity goldens and their runner, when the wasm exports them. */
  readonly parity?: CoreParity;
}

export class CoreUnavailableError extends Error {
  constructor(reason: string) {
    super(`the app core is not loaded: ${reason}`);
    this.name = "CoreUnavailableError";
  }
}

export function unavailableCore(reason: string, status: CoreStatus = "unavailable"): CoreClient {
  return {
    status,
    reason,
    methods: [],
    call() {
      throw new CoreUnavailableError(reason);
    },
    tryCall() {
      return undefined;
    },
  };
}

/** A client over an initialised wasm module. */
export function wasmCore(mod: Pick<CoreWasmModule, "call" | "methods" | "flows" | "runFlow">): CoreClient {
  const methods = JSON.parse(mod.methods()) as string[];
  const known = new Set(methods);
  const call = <T>(method: string, args: Record<string, unknown> = {}): T =>
    JSON.parse(mod.call(method, JSON.stringify(args))) as T;
  return {
    status: "ready",
    methods,
    call,
    tryCall: <T>(method: string, args?: Record<string, unknown>) => (known.has(method) ? call<T>(method, args) : undefined),
    parity:
      mod.flows && mod.runFlow
        ? {
            flows: () => JSON.parse(mod.flows!()) as ParityFlow[],
            run: (name, flow, host) =>
              JSON.parse(mod.runFlow!(name, flow, (m, a) => JSON.stringify(host(m, JSON.parse(a) as Record<string, unknown>)))),
          }
        : undefined,
  };
}

// A glob, not a plain import: it resolves to nothing when ./wasm was not
// built, so the app still builds and runs (demo mode, TS fallbacks).
const loaders = import.meta.glob<CoreWasmModule>("./wasm/core.js");

let loading: Promise<CoreClient> | null = null;

/** Loads the wasm core once (browser, Tauri, Electron, WKWebView). */
export function loadCore(): Promise<CoreClient> {
  loading ??= (async () => {
    const load = loaders["./wasm/core.js"];
    if (!load) return unavailableCore("not built (run scripts/build-core-wasm.mjs)");
    try {
      const mod = await load();
      await mod.default();
      return wasmCore(mod);
    } catch (e) {
      return unavailableCore(e instanceof Error ? e.message : String(e));
    }
  })();
  return loading;
}

/** Loads the core from bytes, synchronously (tests, Node). */
export function coreFromBytes(mod: CoreWasmModule, bytes: BufferSource): CoreClient {
  mod.initSync({ module: bytes });
  return wasmCore(mod);
}
