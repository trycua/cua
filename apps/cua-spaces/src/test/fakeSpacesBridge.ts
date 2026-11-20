// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFallbackSpacesBridge, type SpacesBridge } from "../native/spacesBridge";

/**
 * A SpacesBridge for tests: every command rejects or no-ops like the browser
 * fallback, then `overrides` supplies what a test actually drives.
 */
export function fakeSpacesBridge(overrides: Partial<SpacesBridge> = {}): SpacesBridge {
  return { ...createFallbackSpacesBridge(), ...overrides };
}
