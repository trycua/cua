// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { hostOs, osOfPlatform, THIS_MACHINE, type HostOs } from "@/bridge/host-os";

import { isMacPlatform } from "./utils";

export { hostOs, osOfPlatform, type HostOs };

/** What the app calls the computer it runs on: "This Mac", "This PC" on
 * Windows, "This computer" on Linux. `platform` is a platform word (see
 * {@link osOfPlatform}); without one, the host's. */
export function thisComputerLabel(platform?: string): string {
  return THIS_MACHINE[platform === undefined ? hostOs() : osOfPlatform(platform)];
}

/** {@link thisComputerLabel} mid-sentence: "this Mac", "this PC", "this computer". */
export function thisComputerText(platform?: string): string {
  const label = thisComputerLabel(platform);
  return label.charAt(0).toLowerCase() + label.slice(1);
}

/**
 * The menu bar switch and the global shortcut belong to the macOS app (the
 * notch, ⌘⇧Space). The Windows and Linux shells have a tray icon and no
 * global shortcut, so Settings leaves both rows out there.
 */
export function showsMacShellSettings(platform?: string): boolean {
  return isMacPlatform(platform);
}

const osOf = (platform?: string): HostOs => (platform === undefined ? hostOs() : osOfPlatform(platform));

/** The core's line for a Space being created here ("This Mac · Downloading
 * image…"), naming this computer in the words of its system. */
export function creatingLine(detail: string, platform?: string): string {
  const os = osOf(platform);
  return os === "macos" ? detail : detail.replace(/^This Mac(?= \u00b7 )/, THIS_MACHINE[os]);
}

/** What keeps the Keyvault's key besides a passphrase: "Touch ID",
 * "Windows Hello", "the system keyring" (Linux). */
export function keyvaultUnlockWay(platform?: string): string {
  return { macos: "Touch ID", windows: "Windows Hello", linux: "the system keyring" }[osOf(platform)];
}
