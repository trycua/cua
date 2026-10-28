// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** Settings' tabs, in order. Account is the page at `/settings`. */
export const SETTINGS_TABS = [
  { to: "/settings", label: "Account", exact: true },
  { to: "/settings/general", label: "General", exact: false },
  { to: "/settings/connectors", label: "Connectors", exact: false },
  { to: "/settings/devices", label: "Trusted machines", exact: false },
  { to: "/settings/advanced", label: "Advanced", exact: false },
  { to: "/settings/about", label: "About", exact: false },
] as const;
