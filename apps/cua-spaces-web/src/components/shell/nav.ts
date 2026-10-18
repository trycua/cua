// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { BellIcon, BotIcon, HardDriveIcon, KeyRoundIcon, LayoutGridIcon, ServerIcon, SettingsIcon, type LucideIcon } from "lucide-react";

import { useExperimentFlags } from "@/bridge";
import type { KeybindingCommand } from "@/lib/keybindings";

export interface NavItem {
  to: "/spaces" | "/machines" | "/agents" | "/volume" | "/keyvault" | "/notifications" | "/settings";
  label: string;
  icon: LucideIcon;
  /** Its keyboard shortcut, when it has one. */
  command?: KeybindingCommand;
}

export const NAV_ITEMS: NavItem[] = [
  { to: "/spaces", label: "Spaces", icon: LayoutGridIcon, command: "nav.spaces" },
  { to: "/machines", label: "Machines", icon: ServerIcon, command: "nav.machines" },
  { to: "/agents", label: "Agents", icon: BotIcon, command: "nav.agents" },
  { to: "/keyvault", label: "Keyvault", icon: KeyRoundIcon, command: "nav.keyvault" },
  { to: "/notifications", label: "Notifications", icon: BellIcon },
  { to: "/settings", label: "Settings", icon: SettingsIcon, command: "nav.settings" },
];

/** The sidebar's Volume entry (the core's `volumeLabel`), after Agents. */
const VOLUME_ITEM: NavItem = { to: "/volume", label: "Volume", icon: HardDriveIcon };

/** The nav, with Volume while the Cua Volume experiment is on. */
export function useNavItems(): NavItem[] {
  const experiments = useExperimentFlags();
  if (!experiments?.cuaVolume) return NAV_ITEMS;
  const i = NAV_ITEMS.findIndex((n) => n.to === "/agents") + 1;
  return [...NAV_ITEMS.slice(0, i), VOLUME_ITEM, ...NAV_ITEMS.slice(i)];
}
