// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import type { Settings, SwiftMigration } from "../src/settings";
import { autoCheckPref, autoInstallPref, channelPref, readChannel, type UpdateChannel } from "../src/update-prefs";

const moved = (m: Partial<SwiftMigration> = {}): Settings => ({
  swiftMigration: { at: "2026-10-14T18:05:04.636Z", found: true, copied: ["settings.json"], ...m },
});
const fresh: Settings = { swiftMigration: { at: "2026-10-14T18:05:04.636Z", found: false, copied: [] } };
// macOS before the Sparkle cutover: no channel is served.
const macServed = (_: UpdateChannel) => false;
const everywhere = () => true;

describe("autoCheckPref", () => {
  it("keeps Sparkle's automatic checks on for someone who moved from the Swift app", () => {
    // What a test Mac had after installing the Electron beta over Swift 0.8.0:
    // Sparkle's checks were on by the Info.plist, so the defaults held no key.
    expect(autoCheckPref(moved())).toBe(true);
    expect(autoCheckPref(moved({ sparkleAutomaticChecks: true }))).toBe(true);
  });

  it("keeps them off when the user had turned them off in the Swift app", () => {
    expect(autoCheckPref(moved({ sparkleAutomaticChecks: false }))).toBe(false);
  });

  it("follows the choice made in this app over Sparkle's", () => {
    expect(autoCheckPref({ ...moved(), autoUpdate: false })).toBe(false);
    expect(autoCheckPref({ ...moved({ sparkleAutomaticChecks: false }), autoUpdate: true })).toBe(true);
  });

  it("is on by default for everyone else, and only an explicit off stops it", () => {
    expect(autoCheckPref({})).toBe(true);
    expect(autoCheckPref(fresh)).toBe(true);
    expect(autoCheckPref({}, { CUA_SPACES_AUTO_UPDATE: "1" })).toBe(true);
    expect(autoCheckPref({ autoUpdate: true })).toBe(true);
    // The 0.9.0 default was off and the box was never touched: that stays off only if it was written.
    expect(autoCheckPref({ autoUpdate: false })).toBe(false);
    expect(autoCheckPref({ ...fresh, autoUpdate: false })).toBe(false);
  });
});

describe("autoInstallPref", () => {
  it("applies a downloaded update when the app quits, unless the user turned that off", () => {
    expect(autoInstallPref({})).toBe(true);
    expect(autoInstallPref(fresh)).toBe(true);
    // Sparkle's "download automatically" was a different question; it is not carried over.
    expect(autoInstallPref(moved())).toBe(true);
    expect(autoInstallPref(moved({ sparkleAutomaticDownloads: false }))).toBe(true);
    expect(autoInstallPref({ autoInstall: false })).toBe(false);
    expect(autoInstallPref({ autoInstall: true })).toBe(true);
  });
});

describe("channelPref", () => {
  it("uses the build's channel when nothing is saved", () => {
    expect(channelPref({}, "nightly", everywhere)).toBe("nightly");
    expect(channelPref({}, "stable", everywhere)).toBe("stable");
  });

  it("moves a saved Beta to Nightly", () => {
    expect(readChannel("beta")).toBe("nightly");
    expect(readChannel("nightly")).toBe("nightly");
    expect(readChannel("stable")).toBe("stable");
    expect(readChannel("weekly")).toBeUndefined();
    expect(channelPref({ updateChannel: "beta", updateChannelChosen: true }, "stable", everywhere)).toBe("nightly");
    expect(channelPref({ ...moved(), updateChannel: "beta" }, "stable", everywhere)).toBe("nightly");
  });

  it("drops a carried-over channel this app isn't served on", () => {
    // Swift's "stable" on a macOS Electron build: no Electron feed on macOS yet.
    expect(channelPref({ ...moved(), updateChannel: "stable" }, "nightly", macServed)).toBe("nightly");
    // Where it is served, the Swift choice holds.
    expect(channelPref({ ...moved(), updateChannel: "stable" }, "nightly", everywhere)).toBe("stable");
  });

  it("keeps a channel the user picked in this app", () => {
    expect(channelPref({ updateChannel: "stable", updateChannelChosen: true }, "nightly", macServed)).toBe("stable");
  });
});
