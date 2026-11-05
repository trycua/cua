// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { BellIcon, BotIcon, HardDriveIcon, HouseIcon, KeyRoundIcon, LayoutGridIcon, MousePointer2Icon, ServerIcon, SettingsIcon, WorkflowIcon, type LucideIcon } from "lucide-react";

import { useDriver, useExperimentFlags, useFlag, useSpacesTree } from "@/bridge";
import type { KeybindingCommand } from "@/lib/keybindings";

export interface NavItem {
  to: "/home" | "/spaces" | "/machines" | "/driver" | "/ci" | "/agents" | "/volume" | "/keyvault" | "/notifications" | "/settings";
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

/** The sidebar's CI Runners entry, after Machines, while the CI runners experiment is on. */
const CI_ITEM: NavItem = { to: "/ci", label: "CI Runners", icon: WorkflowIcon };

/** The "Ask first" Home, first while its experiment flag is on (`spaces_ask_first_home`). */
const HOME_ITEM: NavItem = { to: "/home", label: "Home", icon: HouseIcon };

/** Cua Driver, under Machines, while this Mac's Cua Spaces carries the driver (CUA-2706). */
const DRIVER_ITEM: NavItem = { to: "/driver", label: "Cua Driver", icon: MousePointer2Icon };

/**
 * The nav, with Cua Driver while this Mac has it, Volume while the Cua Volume experiment is on, CI Runners while its
 * experiment is, and Home while the Ask-first flag is. With Spaces as a tree (Labs) your computers are in the Spaces
 * list, so Machines leaves the nav (its page still opens); Cua Driver keeps its place.
 */
export function useNavItems(): NavItem[] {
  const experiments = useExperimentFlags();
  const home = useFlag("spaces_ask_first_home");
  const driver = useDriver().available;
  const tree = useSpacesTree();
  let items = NAV_ITEMS;
  if (driver) {
    const i = items.findIndex((n) => n.to === "/machines") + 1;
    items = [...items.slice(0, i), DRIVER_ITEM, ...items.slice(i)];
  }
  if (tree) items = items.filter((n) => n.to !== "/machines");
  if (experiments?.cuaVolume) {
    const i = items.findIndex((n) => n.to === "/agents") + 1;
    items = [...items.slice(0, i), VOLUME_ITEM, ...items.slice(i)];
  }
  if (experiments?.ciRunners) {
    const i = items.findIndex((n) => n.to === (tree ? "/spaces" : "/machines")) + 1;
    items = [...items.slice(0, i), CI_ITEM, ...items.slice(i)];
  }
  return home ? [HOME_ITEM, ...items] : items;
}
