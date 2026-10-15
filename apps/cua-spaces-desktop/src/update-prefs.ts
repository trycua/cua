// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// What the updater (updater.ts) does when the user hasn't chosen in this
// app. Kept free of Electron so it can be tested on its own.
//
// Someone who moved over from the Swift app (migrate-swift.ts) keeps what
// Sparkle did for them there: automatic checks stay on unless they had turned
// them off (the Swift app's Info.plist sets SUEnableAutomaticChecks, so the
// defaults domain only holds the key when the user changed it), and
// automatic installs stay as they had them. Everyone else stays opt-in.
import type { Settings } from "./settings";

export type UpdateChannel = "stable" | "beta";

const movedFromSwift = (s: Settings) => s.swiftMigration?.found === true;

/** Automatic checks: the choice made in this app, else the one carried over from Sparkle, else off. */
export function autoCheckPref(s: Settings, env: NodeJS.ProcessEnv = {}): boolean {
  if (env.CUA_SPACES_AUTO_UPDATE === "1") return true;
  if (typeof s.autoUpdate === "boolean") return s.autoUpdate;
  return movedFromSwift(s) && s.swiftMigration?.sparkleAutomaticChecks !== false;
}

/** Automatic installs: the choice made in this app, else the one carried over from Sparkle (off by default there too). */
export function autoInstallPref(s: Settings): boolean {
  if (typeof s.autoInstall === "boolean") return s.autoInstall;
  return movedFromSwift(s) && s.swiftMigration?.sparkleAutomaticDownloads === true;
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
