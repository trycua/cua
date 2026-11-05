// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { BellIcon, BotIcon, HardDriveIcon, HouseIcon, KeyRoundIcon, MonitorIcon, SettingsIcon, WorkflowIcon, type LucideIcon } from "lucide-react";

import { useExperimentFlags, useFlag } from "@/bridge";
import type { KeybindingCommand } from "@/lib/keybindings";

export interface NavItem {
  to: "/home" | "/spaces" | "/ci" | "/agents" | "/volume" | "/keyvault" | "/notifications" | "/settings";
  label: string;
  icon: LucideIcon;
  /** Its keyboard shortcut, when it has one. */
  command?: KeybindingCommand;
}

export const NAV_ITEMS: NavItem[] = [
  // Your computers and the Spaces made on them, in one list (CUA-3551).
  { to: "/spaces", label: "Computers", icon: MonitorIcon, command: "nav.spaces" },
  { to: "/agents", label: "Agents", icon: BotIcon, command: "nav.agents" },
  { to: "/keyvault", label: "Keyvault", icon: KeyRoundIcon, command: "nav.keyvault" },
  { to: "/notifications", label: "Notifications", icon: BellIcon },
  { to: "/settings", label: "Settings", icon: SettingsIcon, command: "nav.settings" },
];

/** The sidebar's Volume entry (the core's `volumeLabel`), after Agents. */
const VOLUME_ITEM: NavItem = { to: "/volume", label: "Volume", icon: HardDriveIcon };

/** The sidebar's CI Runners entry, after Computers, while the CI runners experiment is on. */
const CI_ITEM: NavItem = { to: "/ci", label: "CI Runners", icon: WorkflowIcon };

/** The "Ask first" Home, first while its experiment flag is on (`spaces_ask_first_home`). */
const HOME_ITEM: NavItem = { to: "/home", label: "Home", icon: HouseIcon };

/**
 * The nav, with Volume while the Cua Volume experiment is on, CI Runners while its experiment is, and Home while the
 * Ask-first flag is. Cua Driver is one of This Mac's settings, on its page under Computers.
 */
export function useNavItems(): NavItem[] {
  const experiments = useExperimentFlags();
  const home = useFlag("spaces_ask_first_home");
  let items = NAV_ITEMS;
  if (experiments?.cuaVolume) {
    const i = items.findIndex((n) => n.to === "/agents") + 1;
    items = [...items.slice(0, i), VOLUME_ITEM, ...items.slice(i)];
  }
  if (experiments?.ciRunners) {
    const i = items.findIndex((n) => n.to === "/spaces") + 1;
    items = [...items.slice(0, i), CI_ITEM, ...items.slice(i)];
  }
  return home ? [HOME_ITEM, ...items] : items;
}
