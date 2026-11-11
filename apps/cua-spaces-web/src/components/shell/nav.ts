// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { BellIcon, BotIcon, HardDriveIcon, HouseIcon, KeyRoundIcon, MonitorIcon, SettingsIcon, WorkflowIcon, type LucideIcon } from "lucide-react";

import { useExperimentFlags } from "@/bridge";
import { useT } from "@/i18n";
import type { KeybindingCommand } from "@/lib/keybindings";

export interface NavItem {
  to: "/home" | "/spaces" | "/ci" | "/agents" | "/volume" | "/keyvault" | "/notifications" | "/settings";
  /** Resolved at render (`useNavItems`), in the language in use. */
  label: string;
  icon: LucideIcon;
  /** Its keyboard shortcut, when it has one. */
  command?: KeybindingCommand;
}

type NavEntry = Omit<NavItem, "label"> & { labelKey: string };

const NAV_ENTRIES: NavEntry[] = [
  // Your computers and the Spaces made on them, in one list (CUA-3551).
  { to: "/spaces", labelKey: "shell.nav.computers", icon: MonitorIcon, command: "nav.spaces" },
  { to: "/agents", labelKey: "shell.nav.agents", icon: BotIcon, command: "nav.agents" },
  { to: "/keyvault", labelKey: "shell.nav.keyvault", icon: KeyRoundIcon, command: "nav.keyvault" },
  { to: "/notifications", labelKey: "shell.nav.notifications", icon: BellIcon },
  { to: "/settings", labelKey: "shell.nav.settings", icon: SettingsIcon, command: "nav.settings" },
];

/** The sidebar's Volume entry (the core's `volumeLabel`), after Agents. */
const VOLUME_ITEM: NavEntry = { to: "/volume", labelKey: "shell.nav.volume", icon: HardDriveIcon };

/** The sidebar's CI Runners entry, after Computers, while the CI runners experiment is on. */
const CI_ITEM: NavEntry = { to: "/ci", labelKey: "shell.nav.ci_runners", icon: WorkflowIcon };

/** Home, always first. */
const HOME_ITEM: NavEntry = { to: "/home", labelKey: "shell.nav.home", icon: HouseIcon };

/**
 * The nav: Home first, then the rest, with Volume while the Cua Volume experiment is on and CI Runners while its
 * experiment is. Cua Driver is one of This Mac's settings, on its page; the sidebar draws its line under Computers
 * (`SidebarDriverLine`).
 */
export function useNavItems(): NavItem[] {
  const t = useT();
  const experiments = useExperimentFlags();
  let items = NAV_ENTRIES;
  if (experiments?.cuaVolume) {
    const i = items.findIndex((n) => n.to === "/agents") + 1;
    items = [...items.slice(0, i), VOLUME_ITEM, ...items.slice(i)];
  }
  if (experiments?.ciRunners) {
    const i = items.findIndex((n) => n.to === "/spaces") + 1;
    items = [...items.slice(0, i), CI_ITEM, ...items.slice(i)];
  }
  return [HOME_ITEM, ...items].map(({ labelKey, ...item }) => ({ ...item, label: t(labelKey) }));
}
