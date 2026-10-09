// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Errors in words, as the SwiftUI app's `LiveSpacesBackend.words` shows
// them: every `CuaError` carries one message, which the generated bindings
// prefix with `CuaError.<Variant>: `.

const UNIFFI_TYPE = Symbol.for("typeName");

export function words(error: unknown): string {
  if (error instanceof Error) {
    if (UNIFFI_TYPE in error) return error.message.replace(/^\w+\.\w+(: )?/, "") || error.message;
    if (error.message) return error.message;
  }
  return String(error);
}

/** The SDK error's variant (`Cancelled`, `Unauthenticated`, ...), if it is one. */
export function sdkErrorKind(error: unknown): string | null {
  if (!(error instanceof Error) || !(UNIFFI_TYPE in error)) return null;
  const tag = (error as { tag?: unknown }).tag;
  return typeof tag === "string" ? tag : null;
}

/** A create ended because it was cancelled (here, or `cua spaces cancel`), not because it failed. */
export const isCancelled = (error: unknown) => sdkErrorKind(error) === "Cancelled";

/** A failed call is the daemon's own answer, not a connection that broke (only that one is made again). */
export function daemonAnswered(error: unknown): boolean {
  const kind = sdkErrorKind(error);
  return kind !== null && kind !== "DaemonNotRunning" && kind !== "Transport" && kind !== "Closed";
}
