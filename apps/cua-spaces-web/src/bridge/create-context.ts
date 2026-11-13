// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { Location } from "./contracts/spaces";

/**
 * Where a create ran, kept with the error it ended in. The core words a
 * failure by who it is about (this Mac, or another machine of yours by
 * name), so the notice a person sees after a failed create needs the
 * create's place as well as the SDK's error.
 */
export interface CreateContext {
  provider: Location | "relay";
  /** The machine's name, for a create on one of your machines. */
  hostName?: string;
}

const contexts = new WeakMap<Error, CreateContext>();

/** Marks `error` as the end of a create that ran at `context`; returns it. */
export function withCreateContext<E extends Error>(error: E, context: CreateContext): E {
  contexts.set(error, context);
  return error;
}

/** Where the create that ended in `error` ran, when it is known. */
export function createContextOf(error: unknown): CreateContext | undefined {
  return error instanceof Error ? contexts.get(error) : undefined;
}
