// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// macOS: Chromium keeps its own key for encrypting cookies in the login
// keychain ("Cua Spaces Safe Storage"). A build whose code signature the
// item's access list doesn't name (in practice every update) makes macOS ask
// for the login keychain password. While that prompt is up, Security holds
// its process-wide lock, so the app's own keychain reads on the main thread
// (auth status) wait behind it and the whole app stops responding until the
// prompt is answered.
//
// The app keeps nothing secret in Chromium's storage: sign-in and Keyvault
// secrets live in the cua keychain items, read by the Rust core, and the
// pages only store the theme, the checklist and onboarding progress (in
// localStorage, which Chromium never encrypts anyway). So Chromium gets a
// fixed key instead (`--use-mock-keychain`) and never touches the keychain.
// No Electron here.

/** The command-line switches that keep Chromium out of the keychain on `platform`. */
export function chromiumKeychainSwitches(platform: NodeJS.Platform): string[] {
  return platform === "darwin" ? ["use-mock-keychain"] : [];
}

/** Applies them to Electron's command line (before the app is ready). */
export function applyChromiumKeychainSwitches(commandLine: { appendSwitch(name: string): void }, platform: NodeJS.Platform): void {
  for (const name of chromiumKeychainSwitches(platform)) commandLine.appendSwitch(name);
}
