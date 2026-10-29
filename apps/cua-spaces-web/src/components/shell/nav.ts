// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { BellIcon, BotIcon, HardDriveIcon, HouseIcon, KeyRoundIcon, LayoutGridIcon, MousePointer2Icon, ServerIcon, SettingsIcon, type LucideIcon } from "lucide-react";

import { useDriver, useExperimentFlags, useFlag } from "@/bridge";
import type { KeybindingCommand } from "@/lib/keybindings";

export interface NavItem {
  to: "/home" | "/spaces" | "/machines" | "/driver" | "/agents" | "/volume" | "/keyvault" | "/notifications" | "/settings";
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

/** The "Ask first" Home, first while its experiment flag is on (`spaces_ask_first_home`). */
const HOME_ITEM: NavItem = { to: "/home", label: "Home", icon: HouseIcon };

/** Cua Driver, under Machines, while this Mac runs the embedded driver (`spaces_embedded_driver`, CUA-2706). */
const DRIVER_ITEM: NavItem = { to: "/driver", label: "Cua Driver", icon: MousePointer2Icon };

/** The nav, with Volume while the Cua Volume experiment is on, Home while the Ask-first flag is, and Cua Driver while this Mac has it. */
export function useNavItems(): NavItem[] {
  const experiments = useExperimentFlags();
  const home = useFlag("spaces_ask_first_home");
  const driver = useDriver().available;
  let items = NAV_ITEMS;
  if (driver) {
    const i = items.findIndex((n) => n.to === "/machines") + 1;
    items = [...items.slice(0, i), DRIVER_ITEM, ...items.slice(i)];
  }
  if (experiments?.cuaVolume) {
    const i = items.findIndex((n) => n.to === "/agents") + 1;
    items = [...items.slice(0, i), VOLUME_ITEM, ...items.slice(i)];
  }
  return home ? [HOME_ITEM, ...items] : items;
}
