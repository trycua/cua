// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { createFallbackFleetBridge, type FleetBridge } from "../native/fleet";

/**
 * A FleetBridge for tests: every command rejects or no-ops like the browser
 * fallback, then `overrides` supplies what a test actually drives.
 */
export function fakeFleetBridge(overrides: Partial<FleetBridge> = {}): FleetBridge {
  return { ...createFallbackFleetBridge(), ...overrides };
}
