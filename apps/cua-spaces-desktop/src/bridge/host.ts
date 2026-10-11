// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The bridge's routing: a method table keyed by the SwiftUI host's method
// names (`WebUIBridge.methods`, `webkit-protocol.ts` in the web bridge), and
// its envelope:
//
//     { id, method, args? }  ->  { id, ok: true, result } | { id, ok: false, error: { code, message } }
//
// `code` is `unimplemented` (no route), `bad_args`, `not_found`,
// `unsupported` (the app can't answer it here), `cancelled`, `native_only`,
// `forbidden` or `failed`. A failure worded for people (a host setup or This
// machine button) also carries `title`, `details` and `actionLabel`. No
// Electron in this file: the main process hands requests in (`ipc.ts`).
import { words } from "../model/errors";

export { sdkErrorKind, words } from "../model/errors";

export type BridgeArgs = Record<string, unknown>;

/** Who asked: the window the request came from (the `window.*` methods act
 * on it), when it came from one. */
export interface Caller {
  readonly window?: unknown;
}

export type Handler = (args: BridgeArgs, caller: Caller) => unknown;
export type Handlers = Record<string, Handler>;

/** How a failure is worded for people (the SwiftUI app's `HostSetupFailure`). */
export interface PresentedFailure {
  title: string;
  details: string;
  actionLabel: string;
}

/** A method's failure, with the code the page reads. */
export class Failure extends Error {
  readonly code: string;
  readonly presented?: PresentedFailure;
  constructor(code: string, message: string, presented?: PresentedFailure) {
    super(message);
    this.name = "Failure";
    this.code = code;
    this.presented = presented;
  }
  static unimplemented(method: string): Failure {
    return new Failure("unimplemented", `${method} is not available in this host`);
  }
  static badArgs(message: string): Failure {
    return new Failure("bad_args", message);
  }
  static notFound(message: string): Failure {
    return new Failure("not_found", message);
  }
  static unsupported(message: string): Failure {
    return new Failure("unsupported", message);
  }
  static failed(message: string): Failure {
    return new Failure("failed", message);
  }
}

export interface BridgeRequest {
  id: string;
  method: string;
  args?: BridgeArgs;
}

export interface BridgeError {
  code: string;
  message: string;
  title?: string;
  details?: string;
  actionLabel?: string;
}

export type BridgeResponse = { id: string; ok: true; result: unknown } | { id: string; ok: false; error: BridgeError };

export function reply(id: string, error: Failure): BridgeResponse {
  const e: BridgeError = { code: error.code, message: error.message };
  if (error.presented) {
    e.title = error.presented.title;
    e.details = error.presented.details;
    e.actionLabel = error.presented.actionLabel;
  }
  return { id, ok: false, error: e };
}

/** The method table. */
export class BridgeRegistry {
  private readonly table = new Map<string, Handler>();

  /** Adds an area's methods; a method registered twice is a mistake. */
  register(handlers: Handlers): void {
    for (const [method, handler] of Object.entries(handlers)) {
      if (this.table.has(method)) throw new Error(`bridge method ${method} is registered twice`);
      this.table.set(method, handler);
    }
  }

  get methods(): string[] {
    return [...this.table.keys()];
  }

  has(method: string): boolean {
    return this.table.has(method);
  }

  /** The method's answer (`undefined` reads as `null`), or its `Failure`. */
  async handle(method: string, args: BridgeArgs = {}, caller: Caller = {}): Promise<unknown> {
    const handler = this.table.get(method);
    if (!handler) throw Failure.unimplemented(method);
    const result = await handler(args, caller);
    return result === undefined ? null : result;
  }

  /** A request from the page, answered in the envelope; never throws. */
  async dispatch(request: unknown, caller: Caller = {}): Promise<BridgeResponse> {
    const r = (request && typeof request === "object" ? request : {}) as Partial<BridgeRequest>;
    const id = typeof r.id === "string" ? r.id : r.id === undefined ? "" : String(r.id);
    if (typeof r.method !== "string") return reply(id, Failure.badArgs("missing method"));
    const args = r.args && typeof r.args === "object" && !Array.isArray(r.args) ? r.args : {};
    try {
      return { id, ok: true, result: await this.handle(r.method, args, caller) };
    } catch (error) {
      return reply(id, error instanceof Failure ? error : new Failure("failed", words(error)));
    }
  }
}

/** Methods that are not answered yet. */
export function unimplemented(...methods: string[]): Handlers {
  return Object.fromEntries(
    methods.map((m) => [
      m,
      () => {
        throw Failure.unimplemented(m);
      },
    ]),
  );
}

/** Pushed events (`cua:event`, `{ event, payload }`). */
export interface BridgeEvent {
  event: string;
  payload?: unknown;
}

export class BridgeEvents {
  private readonly listeners = new Set<(e: BridgeEvent) => void>();

  emit(event: string, payload?: unknown): void {
    const e: BridgeEvent = payload === undefined ? { event } : { event, payload };
    for (const l of [...this.listeners]) {
      try {
        l(e);
      } catch (error) {
        console.error("[cua-spaces] bridge event listener failed", error);
      }
    }
  }

  subscribe(listener: (e: BridgeEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
}
