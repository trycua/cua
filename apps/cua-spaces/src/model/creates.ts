// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Spaces being created (the app core's `spaces::creating`): the row a
 * create shows the instant it starts, fed by the SDK's create progress
 * (`spaces:create-progress`) until the Space is registered. The SwiftUI app
 * runs the same reducer through the cua SDK's bindings.
 */

import { core } from "../core";
import type { Location, Space, SpaceOs } from "./types";

/** One create in flight (or failed). */
export interface PendingCreate {
  id: string;
  name: string;
  os: SpaceOs;
  provider: Location | "relay";
  startedAt: number;
  phase: string;
  fraction?: number | null;
  pulled: boolean;
  permille: number;
  error?: string | null;
  spaceId?: string | null;
  image?: string | null;
  kind?: "container" | "vm" | null;
  arch?: string | null;
  emulated?: boolean;
  phaseAt?: number | null;
  /** GPU acceleration was asked for. */
  gpu?: boolean;
  /** The download's bytes so far, of how many, and its rate (bytes/s). */
  bytesDone?: number | null;
  bytesTotal?: number | null;
  bytesPerSecond?: number | null;
  /** Cancel was pressed: the row shows Cancelling until the create ends. */
  cancelling?: boolean;
}

/** One delete in flight (or done, until the registry drops the Space). */
export interface PendingDelete {
  id: string;
  startedAt: number;
  done: boolean;
}

/** Every pending create and delete. */
/** One power action in flight (or done until the registry shows it, or
 * failed until the next one). */
export interface PendingPower {
  id: string;
  /** Turning it on (else off). */
  on: boolean;
  startedAt: number;
  done: boolean;
  error?: string | null;
}

export interface CreatesState {
  pending: PendingCreate[];
  deleting?: PendingDelete[];
  powering?: PendingPower[];
}

export type CreateAction =
  | {
      type: "start";
      id: string;
      name: string;
      os: SpaceOs;
      provider: Location | "relay";
      now: number;
      /** The image (its catalog entry names the distribution, kind and
       * platforms), the kind asked for, and this Mac's CPU architecture. */
      image?: string | null;
      kind?: "container" | "vm" | null;
      hostArch?: string | null;
      /** GPU acceleration was asked for (the create's `gpu` option). */
      gpu?: boolean;
    }
  | {
      type: "progress";
      id: string;
      phase: string;
      fraction?: number | null;
      now?: number | null;
      /** An image download's bytes so far, of how many, and how fast. */
      bytesDone?: number | null;
      bytesTotal?: number | null;
      bytesPerSecond?: number | null;
    }
  /** A few times a second while a create is pending: progress within a
   * phase without a fraction advances with the time it usually takes. */
  | { type: "tick"; now: number }
  | { type: "finish"; id: string; spaceId: string }
  | { type: "fail"; id: string; error: string }
  | { type: "dismiss"; id: string }
  /** Cancel was pressed (the shell then calls `cancel_create`). */
  | { type: "cancel-start"; id: string }
  /** The cancel finished: the row goes. */
  | { type: "cancel-done"; id: string }
  /** The cancel itself failed: the row says why. */
  | { type: "cancel-fail"; id: string; error: string }
  | { type: "delete-start"; id: string; now: number }
  | { type: "delete-fail"; id: string }
  | { type: "delete-done"; id: string }
  /** The power button was pressed: the row says Suspending (and the like)
   * now; a second press while one runs does nothing. */
  | { type: "power-start"; id: string; on: boolean; now: number }
  /** The SDK's stop or start returned. */
  | { type: "power-done"; id: string }
  /** It failed: the row shows why, inline. */
  | { type: "power-fail"; id: string; error: string };

export const NO_CREATES: CreatesState = { pending: [], deleting: [], powering: [] };

/** Advances the pending creates. */
export function reduceCreates(state: CreatesState, action: CreateAction): CreatesState {
  return core<CreatesState>("creates.reduce", { state, action });
}

/** The registry's Spaces plus a row per pending create. */
export function composeCreates(spaces: Space[], state: CreatesState): Space[] {
  return core<Space[]>("creates.compose", { spaces, state });
}

/** What a cancelled create's error starts with (the SDK's `cancelled`
 * tag, as the shell's `create_space` reports it). */
export const CANCELLED_PREFIX = "cancelled: ";

/** Whether a create's error means it was cancelled, not that it failed. */
export function isCancelledError(error: string): boolean {
  return error.startsWith(CANCELLED_PREFIX);
}

/** Whether a create is still running (a timer should tick). */
export function createsRunning(state: CreatesState): boolean {
  return state.pending.some((p) => !p.error && !p.spaceId);
}

/** Whether `id` is a Space still being created (or whose create failed). */
export function isPendingCreate(id: string): boolean {
  return core<boolean>("creates.isPending", { id });
}

/** Drops finished creates the registry lists and finished deletes it no
 * longer lists (after each registry refresh). */
export function settleCreates(state: CreatesState, spaces: Space[]): CreatesState {
  return core<CreatesState>("creates.settle", { state, spaces });
}

/** Whether the Space `id` is being deleted: a Delete for it does nothing. */
export function isDeleting(state: CreatesState, id: string): boolean {
  return core<boolean>("creates.isDeleting", { state, id });
}

/** Whether a power action runs for the Space `id`: its button waits. */
export function isPowering(state: CreatesState, id: string): boolean {
  return core<boolean>("creates.isPowering", { state, id });
}
