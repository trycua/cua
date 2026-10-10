// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { appCoreStatePaths, boundsFromFrame, migrateFromSwiftApp, parsePlist, swiftSupportDir } from "../src/migrate-swift";

/** A recorded file in test/fixtures. */
const fixture = (name: string) => readFileSync(path.join(__dirname, "fixtures", name), "utf8");

const dirs: string[] = [];
afterEach(() => dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));

/** A throwaway HOME (with the Swift app's support files, optionally) and userData. */
function machine(swift: { settings?: object; onboarding?: object } = {}) {
  const root = mkdtempSync(path.join(tmpdir(), "migrate-swift-"));
  dirs.push(root);
  const home = path.join(root, "home");
  const userData = path.join(root, "userData");
  const support = swiftSupportDir(home);
  mkdirSync(support, { recursive: true });
  if (swift.settings) writeFileSync(path.join(support, "settings.json"), JSON.stringify(swift.settings));
  if (swift.onboarding) writeFileSync(path.join(support, "onboarding.json"), JSON.stringify(swift.onboarding));
  return { home, userData, support };
}

const now = new Date("2026-10-07T12:00:00Z");
const swiftSettings = { hotkey: "⌘⇧K", menuBar: true, updateChannel: "beta", lastSeenVersion: "0.7.2 (0.7.2.512)", experiments: {} };
const onboarding = { completed: true };

describe("parsePlist", () => {
  it("reads what `defaults export` prints", () => {
    const p = parsePlist(fixture("swift-defaults.plist")) as Record<string, unknown>;
    expect(p["NSWindow Frame CuaSpacesWebUI"]).toBe("220 140 1280 800 0 0 1728 1079");
    expect(p.SUEnableAutomaticChecks).toBe(true);
    expect(p.SUAutomaticallyUpdate).toBe(false);
    expect(p.SUUpdateGroupIdentifier).toBe(3920114467);
    expect(p.SULastCheckTime).toBe("2026-10-06T09:12:44Z");
    expect(p.NSNavPanelExpandedStateForSaveMode).toBe("AAE=");
    expect(p.NSRecentDocuments).toEqual(["a & b", {}]);
  });

  it("refuses broken input", () => {
    expect(() => parsePlist("<plist><dict><key>a</key>")).toThrow(/plist/);
    expect(() => parsePlist("<plist><dict><string>x</string></dict></plist>")).toThrow(/key/);
  });
});

describe("boundsFromFrame", () => {
  it("turns AppKit's bottom-left origin into Electron's top-left", () => {
    expect(boundsFromFrame("220 140 1280 800 0 0 1728 1079", 1117)).toEqual({ x: 220, y: 1117 - 940, width: 1280, height: 800 });
  });

  it("ignores frames it cannot use", () => {
    expect(boundsFromFrame("nonsense", 1000)).toBeUndefined();
    expect(boundsFromFrame("0 0 50 50", 1000)).toBeUndefined();
    expect(boundsFromFrame("0 0 800 600", 0)).toBeUndefined();
  });
});

describe("migrateFromSwiftApp", () => {
  it("copies the app core's files and maps the defaults, once", () => {
    const m = machine({ settings: swiftSettings, onboarding });
    const patch = migrateFromSwiftApp({ ...m, settings: {}, defaultsXml: fixture("swift-defaults.plist"), primaryHeight: 1117, now });
    const paths = appCoreStatePaths(m.userData);
    expect(JSON.parse(readFileSync(paths.settings, "utf8"))).toEqual(swiftSettings);
    expect(JSON.parse(readFileSync(paths.onboarding, "utf8"))).toEqual(onboarding);
    expect(patch).toEqual({
      updateChannel: "beta",
      windowBounds: { x: 220, y: 177, width: 1280, height: 800 },
      swiftMigration: {
        at: "2026-10-07T12:00:00.000Z",
        found: true,
        copied: ["settings.json", "onboarding.json"],
        from: "0.7.2 (0.7.2.512)",
        sparkleAutomaticChecks: true,
        sparkleAutomaticDownloads: false,
      },
    });
    // The Swift app's files stay where they are.
    expect(existsSync(path.join(m.support, "settings.json"))).toBe(true);
    // With the marker, nothing runs again.
    expect(migrateFromSwiftApp({ ...m, settings: { swiftMigration: patch!.swiftMigration }, defaultsXml: null, primaryHeight: 1117, now })).toBeNull();
  });

  it("never overwrites what this app already has", () => {
    const m = machine({ settings: swiftSettings, onboarding });
    const paths = appCoreStatePaths(m.userData);
    mkdirSync(m.userData, { recursive: true });
    writeFileSync(paths.settings, '{"hotkey":"mine"}');
    const own = { width: 900, height: 700 };
    const patch = migrateFromSwiftApp({
      ...m,
      settings: { updateChannel: "stable", windowBounds: own },
      defaultsXml: fixture("swift-defaults.plist"),
      primaryHeight: 1117,
      now,
    });
    expect(readFileSync(paths.settings, "utf8")).toBe('{"hotkey":"mine"}');
    expect(patch?.updateChannel).toBeUndefined();
    expect(patch?.windowBounds).toBeUndefined();
    expect(patch?.swiftMigration?.copied).toEqual(["onboarding.json"]);
  });

  it("marks a Mac without the Swift app, and never turns updates on", () => {
    const m = machine();
    const patch = migrateFromSwiftApp({ ...m, settings: {}, defaultsXml: null, primaryHeight: 1117, now });
    expect(patch).toEqual({ swiftMigration: { at: "2026-10-07T12:00:00.000Z", found: false, copied: [] } });
    expect(existsSync(m.userData)).toBe(false);
    const withSparkle = migrateFromSwiftApp({ ...machine(), settings: {}, defaultsXml: fixture("swift-defaults.plist"), primaryHeight: 1117, now });
    expect(withSparkle).not.toHaveProperty("autoUpdate");
  });

  it("counts only the Swift app's own defaults as found", () => {
    const xml = '<plist version="1.0"><dict><key>NSNavLastRootDirectory</key><string>~/Desktop</string></dict></plist>';
    expect(migrateFromSwiftApp({ ...machine(), settings: {}, defaultsXml: xml, primaryHeight: 1117, now })?.swiftMigration?.found).toBe(false);
  });

  it("survives damaged input", () => {
    const m = machine();
    writeFileSync(path.join(m.support, "settings.json"), "{not json");
    const patch = migrateFromSwiftApp({ ...m, settings: {}, defaultsXml: "<plist><dict>", primaryHeight: 1117, now });
    expect(patch?.swiftMigration).toMatchObject({ found: true, copied: ["settings.json"] });
    expect(patch?.updateChannel).toBeUndefined();
  });
});
