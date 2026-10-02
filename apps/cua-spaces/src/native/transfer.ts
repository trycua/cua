// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Typed wrapper over the teleport transfer overlay commands/events (item B).
 *
 * The picker calls `begin` the moment a push starts (instantly opens/focuses the
 * Space window with the overlay up) and then closes — from there the shell (Rust)
 * owns the whole lifecycle, emitting progress and the terminal done/error to the
 * Space window itself. The Space viewer subscribes with `onTransfer`; its Retry
 * calls `retry` (Rust re-runs the stored push) and its Cancel calls `cancel`
 * (Rust dismisses the overlay back to the live stream).
 *
 * Outside Tauri every call is a no-op so the browser/test build stays hermetic.
 */
import type { TransferSignal } from "../model/transfer";
import { hasTauri } from "./bridge";

export interface TransferBridge {
  readonly isNative: boolean;
  /** Open/focus the Space window and show the active transfer overlay. */
  begin(spaceId: string, spaceName: string, appName: string): Promise<void>;
  /** Update an in-flight transfer: "done" clears it, "error" shows Retry. */
  update(spaceId: string, status: "done" | "error", message?: string): Promise<void>;
  /** Re-run a Space's push in Rust from its stored params (the overlay's Retry). */
  retry(spaceId: string): Promise<void>;
  /** Dismiss a Space's transfer overlay back to the live stream (the Cancel). */
  cancel(spaceId: string): Promise<void>;
  /** Viewer window: subscribe to transfer lifecycle signals for its Space. */
  onTransfer(handler: (signal: TransferSignal) => void): Promise<() => void>;
}

const noopUnsub = async () => () => {};

export function createFallbackTransferBridge(): TransferBridge {
  return {
    isNative: false,
    begin: async () => {},
    update: async () => {},
    retry: async () => {},
    cancel: async () => {},
    onTransfer: noopUnsub,
  };
}

export function createTauriTransferBridge(): TransferBridge {
  const core = import("@tauri-apps/api/core");
  const event = import("@tauri-apps/api/event");
  const invoke = async <T>(command: string, args?: Record<string, unknown>) =>
    (await core).invoke<T>(command, args);
  return {
    isNative: true,
    begin: (spaceId, spaceName, appName) =>
      invoke<void>("begin_space_transfer", { request: { spaceId, spaceName, appName } }),
    update: (spaceId, status, message) =>
      invoke<void>("update_space_transfer", { spaceId, status, message: message ?? null }),
    retry: (spaceId) => invoke<void>("retry_space_transfer", { spaceId }),
    cancel: (spaceId) => invoke<void>("cancel_space_transfer", { spaceId }),
    onTransfer: async (handler) => {
      const { listen } = await event;
      return listen<TransferSignal>("space-transfer", (e) => handler(e.payload));
    },
  };
}

export function createTransferBridge(): TransferBridge {
  return hasTauri() ? createTauriTransferBridge() : createFallbackTransferBridge();
}
