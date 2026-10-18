// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The build Settings → About shows after the version (bundle-version.ts):
// the packaged Mac app's CFBundleVersion, as the SwiftUI app's About shows
// its own ("Version 0.7.2 (0.7.2.41)"); none in development or elsewhere.
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { appBuild, bundleVersionFromPlist } from "../src/bundle-version";

const plist = `<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>CFBundleShortVersionString</key>
  <string>0.7.2</string>
  <key>CFBundleVersion</key>
  <string>0.7.2.41</string>
</dict></plist>`;

describe("the app's build", () => {
  it("reads CFBundleVersion from the bundle's Info.plist", () => {
    expect(bundleVersionFromPlist(plist)).toBe("0.7.2.41");
    expect(bundleVersionFromPlist("<plist><dict/></plist>")).toBe("");
  });

  it("is the packaged Mac app's, next to its Resources, and none in development, elsewhere or when unreadable", () => {
    const read = (file: string) => {
      expect(file).toBe(path.join("/Applications/Cua Spaces.app/Contents", "Info.plist"));
      return plist;
    };
    const resourcesPath = "/Applications/Cua Spaces.app/Contents/Resources";
    expect(appBuild({ platform: "darwin", packaged: true, resourcesPath, read })).toBe("0.7.2.41");
    expect(appBuild({ platform: "darwin", packaged: false, resourcesPath, read })).toBe("");
    expect(appBuild({ platform: "win32", packaged: true, resourcesPath, read })).toBe("");
    expect(
      appBuild({
        platform: "darwin",
        packaged: true,
        resourcesPath,
        read: () => {
          throw new Error("ENOENT");
        },
      }),
    ).toBe("");
  });
});
