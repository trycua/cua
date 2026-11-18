// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The app core's preparing state machine (`prepare.reduce`, `prepare.view`,
 * `prepare.metrics`; `cua-spaces-app-core/src/prepare.rs`). Without the core
 * there is nothing to prepare: every call answers `null`.
 */

import type { CoreClient } from "./core";
import type { PrepareAction, PrepareMetrics, PrepareReduced, PrepareState, PrepareView } from "./contracts/prepare";

export function prepareReduce(core: CoreClient, state: PrepareState | null, action: PrepareAction): PrepareReduced | null {
  return core.tryCall<PrepareReduced>("prepare.reduce", { state, action }) ?? null;
}

export function prepareView(core: CoreClient, state: PrepareState | null): PrepareView | null {
  return core.tryCall<PrepareView>("prepare.view", { state }) ?? null;
}

export function prepareMetrics(core: CoreClient, state: PrepareState | null): PrepareMetrics | null {
  return core.tryCall<PrepareMetrics>("prepare.metrics", { state }) ?? null;
}

/** The Spaces no list shows (`prepare.hidden`): the home warming for Home's box until a person wrote into it. */
export function prepareHidden(core: CoreClient, state: PrepareState | null): string[] {
  return core.tryCall<string[]>("prepare.hidden", { state }) ?? [];
}

/** `ids` as a list shows them (`prepare.listed`): without the hidden ones. */
export function prepareListed(core: CoreClient, state: PrepareState | null, ids: string[]): string[] {
  return core.tryCall<string[]>("prepare.listed", { state, ids }) ?? ids;
}
