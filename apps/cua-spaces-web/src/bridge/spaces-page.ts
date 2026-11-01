// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The new Spaces page's decisions (CUA-2204), each one call into the app
 * core: a Space's "…" menu, the bulk delete and leave dialog with its
 * progress and done toast, tips, the Pro banner and the account menu. The
 * words and rules are the core's (the SwiftUI and Electron apps call the
 * same functions over UniFFI); this draws nothing.
 *
 * Tips are kept in the core's app settings (`AppSettings.tips`): change
 * them with `tipsReduce` / `chooseSettings` and save the result with the
 * host's `settings.set`.
 */

import type { CoreClient } from "./core";
import type {
  AccountMenuInput,
  AccountMenuItem,
  BulkConfirmView,
  BulkDone,
  BulkItem,
  BulkResult,
  CoreAppSettings,
  ProBanner,
  ProBannerInput,
  SpaceMenuInput,
  SpaceMenuItem,
  Tip,
  TipFacts,
  TipPlace,
  TipsEvent,
  TipsState,
} from "./contracts/spaces-page";

/** A Space's "…" and right-click menu. */
export const spaceMenu = (core: CoreClient, input: SpaceMenuInput) => core.call<SpaceMenuItem[]>("spaces.menu", { ...input });

/** The confirm dialog for deleting and leaving `items` (null: nothing selected). */
export const bulkConfirm = (core: CoreClient, items: BulkItem[]) => core.call<BulkConfirmView | null>("spaces.bulkConfirm", { items });

/** The selection bar while it runs ("Deleting 1 of 3…"); null once all are finished. */
export const bulkProgress = (core: CoreClient, items: BulkItem[], finished: string[]) =>
  core.call<string | null>("spaces.bulkProgress", { items, finished });

/** The toast once every result is in ("Deleted 3 Spaces. 29 GB freed."). */
export const bulkDone = (core: CoreClient, items: BulkItem[], results: BulkResult[]) => core.call<BulkDone>("spaces.bulkDone", { items, results });

/** The tip for `place` now (null: none). */
export const tipsView = (core: CoreClient, place: TipPlace, facts: TipFacts, state: TipsState) =>
  core.call<Tip | null>("tips.view", { place, facts, state });

/** The tips after `event`. */
export const tipsReduce = (core: CoreClient, state: TipsState, event: TipsEvent) => core.call<TipsState>("tips.reduce", { state, event });

/** The settings after a Settings row the core owns (`tips` on/off, `reset-tips`). */
export const chooseSettings = (core: CoreClient, settings: Partial<CoreAppSettings>, row: string, option: string) =>
  core.call<CoreAppSettings>("settings.choose", { settings, row, option });

/** The Pro banner (Free only; it cannot be closed). */
export const proBanner = (core: CoreClient, input: ProBannerInput) => core.call<ProBanner>("upsell.proBanner", { ...input });

/** The account menu: Settings, Plan (with its word), Sign out. */
export const accountMenu = (core: CoreClient, input: AccountMenuInput) => core.call<AccountMenuItem[]>("account.menu", { ...input });
