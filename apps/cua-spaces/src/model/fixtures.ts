// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { Region, SizeOption, Space } from "./types";

/**
 * Fixed reference time so fixture ordering is deterministic in tests and
 * screenshots. The UI treats it as "now" when seeding state.
 */
export const FIXTURE_NOW = Date.UTC(2026, 4, 13, 16, 41, 0);

const minutesAgo = (m: number) => FIXTURE_NOW - m * 60_000;

/** Synthetic Spaces. None of these correspond to real cloud computers. */
export const FIXTURE_SPACES: Space[] = [
  {
    id: "this-mac",
    name: "This Mac",
    os: "macos",
    status: "local",
    detail: "Local",
    lastUsedAt: minutesAgo(0),
    scene: "mac-desktop",
  },
  {
    id: "windows-qa",
    name: "Windows QA",
    os: "windows",
    status: "running",
    detail: "Agent testing",
    lastUsedAt: minutesAgo(2),
    scene: "windows-desktop",
    size: "standard",
    region: "us-east",
  },
  {
    id: "linux-build",
    name: "Linux Build",
    os: "linux",
    status: "approval",
    detail: "Approval",
    lastUsedAt: minutesAgo(58),
    scene: "linux-terminal",
    size: "large",
    region: "us-central",
  },
  {
    id: "research",
    name: "Research",
    os: "linux",
    status: "suspended",
    detail: "Suspended",
    lastUsedAt: minutesAgo(3 * 24 * 60),
    scene: "notes",
    size: "small",
    region: "eu-west",
  },
];

export const REGIONS: Region[] = [
  { id: "us-east", label: "US East", city: "Virginia" },
  { id: "us-central", label: "US Central", city: "Chicago" },
  { id: "eu-west", label: "EU West", city: "Dublin" },
];

export const SIZES: SizeOption[] = [
  { id: "small", label: "Small", vcpu: 2, memoryGb: 4, linuxHourlyUsd: 0.18 },
  { id: "standard", label: "Standard", vcpu: 4, memoryGb: 8, linuxHourlyUsd: 0.34 },
  { id: "large", label: "Large", vcpu: 8, memoryGb: 16, linuxHourlyUsd: 0.62 },
];
