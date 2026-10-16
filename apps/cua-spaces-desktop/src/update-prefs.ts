// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// What the updater (updater.ts) does when the user hasn't chosen in this
// app. Kept free of Electron so it can be tested on its own.
//
// Automatic checks are on unless the user turned them off in this app, or in
// the Swift app they moved over from (migrate-swift.ts: the Swift app's
// Info.plist sets SUEnableAutomaticChecks, so the defaults domain only holds
// the key when the user changed it). An update found by a check downloads in
// the background; the in-app indicator then offers "Restart to update", and
// nothing restarts without that click.
import type { Settings } from "./settings";

export type UpdateChannel = "stable" | "beta";

const movedFromSwift = (s: Settings) => s.swiftMigration?.found === true;

/** Automatic checks: the choice made in this app, else the one carried over from Sparkle, else on. */
export function autoCheckPref(s: Settings, env: NodeJS.ProcessEnv = {}): boolean {
  if (env.CUA_SPACES_AUTO_UPDATE === "1") return true;
  if (typeof s.autoUpdate === "boolean") return s.autoUpdate;
  return !movedFromSwift(s) || s.swiftMigration?.sparkleAutomaticChecks !== false;
}

/**
 * Install a downloaded update when the app next quits (never a restart on
 * its own): the choice made in this app, else on. Sparkle's "download
 * automatically" was a different question (it is what the update waits for,
 * and now every update downloads in the background), so it is not carried over.
 */
export function autoInstallPref(s: Settings): boolean {
  return typeof s.autoInstall === "boolean" ? s.autoInstall : true;
}

/**
 * The channel to update from. A channel chosen in this app is kept as it
 * is. One carried over from the Swift app that this app can't be served on
 * this platform (Stable on macOS before the Sparkle cutover) would mean no
 * updates at all, so the build's own channel is used instead.
 */
export function channelPref(s: Settings, build: UpdateChannel, served: (channel: UpdateChannel) => boolean): UpdateChannel {
  const saved = s.updateChannel;
  if (!saved) return build;
  if (s.updateChannelChosen || served(saved)) return saved;
  return build;
}
