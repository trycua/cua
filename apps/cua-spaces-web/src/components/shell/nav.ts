// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { BellIcon, BotIcon, HardDriveIcon, HouseIcon, KeyRoundIcon, LayoutGridIcon, ServerIcon, SettingsIcon, type LucideIcon } from "lucide-react";

import { useExperimentFlags, useFlag } from "@/bridge";
import type { KeybindingCommand } from "@/lib/keybindings";

export interface NavItem {
  to: "/home" | "/spaces" | "/machines" | "/agents" | "/volume" | "/keyvault" | "/notifications" | "/settings";
  label: string;
  icon: LucideIcon;
  /** Its keyboard shortcut, when it has one. */
  command?: KeybindingCommand;
}

export const NAV_ITEMS: NavItem[] = [
  { to: "/spaces", label: "Spaces", icon: LayoutGridIcon, command: "nav.spaces" },
  { to: "/machines", label: "Computers", icon: ServerIcon, command: "nav.machines" },
  { to: "/agents", label: "Agents", icon: BotIcon, command: "nav.agents" },
  { to: "/keyvault", label: "Keyvault", icon: KeyRoundIcon, command: "nav.keyvault" },
  { to: "/notifications", label: "Notifications", icon: BellIcon },
  { to: "/settings", label: "Settings", icon: SettingsIcon, command: "nav.settings" },
];

/** The sidebar's Volume entry (the core's `volumeLabel`), after Agents. */
const VOLUME_ITEM: NavItem = { to: "/volume", label: "Volume", icon: HardDriveIcon };

/** The "Ask first" Home, first while its experiment flag is on (`spaces_ask_first_home`). */
const HOME_ITEM: NavItem = { to: "/home", label: "Home", icon: HouseIcon };

/** The nav, with Volume while the Cua Volume experiment is on, and Home while the Ask-first flag is. */
export function useNavItems(): NavItem[] {
  const experiments = useExperimentFlags();
  const home = useFlag("spaces_ask_first_home");
  let items = NAV_ITEMS;
  if (experiments?.cuaVolume) {
    const i = NAV_ITEMS.findIndex((n) => n.to === "/agents") + 1;
    items = [...NAV_ITEMS.slice(0, i), VOLUME_ITEM, ...NAV_ITEMS.slice(i)];
  }
  return home ? [HOME_ITEM, ...items] : items;
}
