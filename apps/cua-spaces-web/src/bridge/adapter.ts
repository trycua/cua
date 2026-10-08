// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { HostEvent, OpArgs, OpName, OpResult } from "./protocol";

export type BridgeMode = "tauri" | "electron" | "webkit" | "demo";

/** The hosts that answer `webkit-protocol.ts`'s methods from the app's own
 * models: the SwiftUI app and the Electron shell. They ask in their own
 * dialogs and post their own notifications. */
export const isNativeHost = (mode: BridgeMode) => mode === "webkit" || mode === "electron";

/**
 * One host behind one interface. `call` answers the operations in
 * `protocol.ts`; `subscribe` delivers the host's pushed events. Hooks never
 * see which host it is, only this.
 */
export interface DataAdapter {
  readonly mode: BridgeMode;
  call<K extends OpName>(op: K, args: OpArgs<K>): Promise<OpResult<K>>;
  /** Pushed events; returns the unsubscribe. */
  subscribe(listener: (event: HostEvent) => void): () => void;
  /**
   * Hosts that can't push registry changes ask to be polled while a hook
   * watches Spaces (milliseconds). Undefined: events only.
   */
  readonly pollSpacesMs?: number;
  /**
   * What the host said about its last registry read, after `spaces.list`:
   * null when it worked, else a line to show above every page while the
   * rows already listed stay (the SwiftUI app's `rosterError`). Undefined:
   * the host says nothing beyond the list (a failed read rejects instead).
   */
  listNotice?(): string | null;
  /** Releases timers and listeners. */
  dispose?(): void;
}

/** An operation the host does not answer. */
export class UnsupportedOperationError extends Error {
  readonly code = "unsupported";
  constructor(mode: BridgeMode, op: string) {
    super(`${op} is not available in ${mode} mode`);
    this.name = "UnsupportedOperationError";
  }
}

/** Whether `e` says the host does not answer an operation (any host's way of saying it). */
export function isUnsupported(e: unknown): boolean {
  return e instanceof UnsupportedOperationError || (e instanceof HostError && e.code === "unsupported");
}

/** A failure a host words for people (the SwiftUI app's `HostSetupFailure`):
 * a short title, the raw error for Details, and the button that retries it
 * ("Retry", or "Sign In" when the account is what is missing). */
export interface PresentedFailure {
  title?: string;
  details?: string;
  actionLabel?: string;
}

/** A host answered with an error. `code` is the host's, when it gave one. */
export class HostError extends Error {
  readonly code?: string;
  /** How the host words it, when it does (host setup and This machine's buttons). */
  readonly presented?: PresentedFailure;
  constructor(message: string, code?: string, presented?: PresentedFailure) {
    super(message);
    this.name = "HostError";
    this.code = code;
    if (presented && (presented.title || presented.details)) this.presented = presented;
  }
}

/** Small fan-out used by adapters that receive events from one source. */
export class Emitter {
  private listeners = new Set<(event: HostEvent) => void>();
  emit(event: HostEvent): void {
    for (const l of [...this.listeners]) {
      try {
        l(event);
      } catch (e) {
        console.error("bridge listener failed", e);
      }
    }
  }
  subscribe(listener: (event: HostEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
  get size(): number {
    return this.listeners.size;
  }
  clear(): void {
    this.listeners.clear();
  }
}
