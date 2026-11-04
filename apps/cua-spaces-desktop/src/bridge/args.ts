// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A method's arguments, checked as the SwiftUI host checks them
// (`WebUIBridge.string`, `strings`, ...): a wrong one fails with `bad_args`
// and says what it takes.
import { Failure, type BridgeArgs } from "./host";

/** A non-empty string. */
export function string(args: BridgeArgs, key: string): string {
  const v = args[key];
  if (typeof v !== "string" || v === "") throw Failure.badArgs(`${key}: string`);
  return v;
}

/** A string, or null when it is absent, null or empty. */
export function optionalString(args: BridgeArgs, key: string): string | null {
  const v = args[key];
  return typeof v === "string" && v !== "" ? v : null;
}

export function strings(args: BridgeArgs, key: string): string[] {
  const v = args[key];
  if (!Array.isArray(v) || !v.every((s) => typeof s === "string")) throw Failure.badArgs(`${key}: string[]`);
  return v as string[];
}

export function bool(args: BridgeArgs, key: string): boolean {
  const v = args[key];
  if (typeof v !== "boolean") throw Failure.badArgs(`${key}: boolean`);
  return v;
}

export function object(args: BridgeArgs, key: string): BridgeArgs {
  const v = args[key];
  if (!v || typeof v !== "object" || Array.isArray(v)) throw Failure.badArgs(`${key}: object`);
  return v as BridgeArgs;
}

/** A whole number at least 0 (Swift's `NSNumber.uint32Value` and kin), or null. */
export function count(args: BridgeArgs, key: string): number | null {
  const v = args[key];
  return typeof v === "number" && Number.isFinite(v) && v >= 0 ? Math.floor(v) : null;
}
