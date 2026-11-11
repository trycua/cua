// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The time the screens put into the app core's words ("saved 2h ago"). One place, so a test can fix it. */

let source: () => number = () => Date.now();

export const now = (): number => source();

/** Tests (and nothing else) fix the clock; `null` puts the real one back. */
export function setClockForTests(f: (() => number) | null): void {
  source = f ?? (() => Date.now());
}
