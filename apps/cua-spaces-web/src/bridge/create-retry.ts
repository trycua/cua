// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * A create that fails for a reason that usually passes runs once more by
 * itself; one that keeps failing the same way offers its fix instead of a
 * plain Try again. The app core decides both (`spaces::creating`: the
 * row's `retryPending` and `fix`, the state's `streaks`); this module holds
 * what the page itself needs around that: which hosts run the retry, where
 * the failure counts are kept between launches, and what a fix button does.
 */

import type { BridgeMode } from './adapter';
import type { CreateFix, FailureStreak } from './contracts/spaces';

/** Hosts whose own model runs every create the page starts (the SwiftUI
 * and Electron apps' `AppModel.runCreate`): they retry a passing failure
 * themselves and tell the page (`CreateProgress.retry`), and they record
 * the create's usage events. Elsewhere the page does both. */
export const HOST_RUNS_CREATES: Record<BridgeMode, boolean> = {
  tauri: false,
  webkit: true,
  electron: true,
  demo: false,
};

const STREAKS_KEY = 'cua.spaces.createStreaks';

/** The failure counts kept from earlier launches (per install: the page's
 * own storage). Only cause words, machine ids and image names. */
export function loadCreateStreaks(): FailureStreak[] {
  try {
    const raw = globalThis.localStorage?.getItem(STREAKS_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (s): s is FailureStreak =>
        !!s &&
        typeof s.kind === 'string' &&
        typeof s.place === 'string' &&
        typeof s.image === 'string' &&
        typeof s.count === 'number'
    );
  } catch {
    return [];
  }
}

export function saveCreateStreaks(streaks: FailureStreak[] | undefined): void {
  try {
    globalThis.localStorage?.setItem(STREAKS_KEY, JSON.stringify(streaks ?? []));
  } catch {
    // Storage off: the counts last until the app quits.
  }
}

/** What a fix button does, through the page's own means. */
export interface CreateFixActions {
  copy(text: string): Promise<void> | void;
  openExternal(url: string): Promise<void> | void;
  details(): void;
}

export async function runCreateFix(fix: CreateFix, act: CreateFixActions): Promise<void> {
  if (fix.action === 'copy-command' && fix.value) await act.copy(fix.value);
  else if (fix.action === 'open-url' && fix.value) await act.openExternal(fix.value);
  else act.details();
}
