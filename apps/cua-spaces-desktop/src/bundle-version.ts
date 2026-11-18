// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The app's build, as Settings → About shows it after the version
// ("Version 0.7.2 (0.7.2.41)"), the Swift app's `CFBundleVersion`: on macOS
// the packaged bundle's Info.plist (electron-builder writes X.Y.Z.<build
// number>, the Swift app's scheme). Elsewhere, and in development, none.
import * as path from "node:path";

/** `CFBundleVersion` from an Info.plist's XML, or "" when it has none. */
export function bundleVersionFromPlist(xml: string): string {
  const m = /<key>CFBundleVersion<\/key>\s*<string>([^<]*)<\/string>/.exec(xml);
  return m?.[1]?.trim() ?? "";
}

/** This build's `CFBundleVersion` (macOS, packaged), else "". */
export function appBuild(o: { platform: NodeJS.Platform; packaged: boolean; resourcesPath: string; read: (file: string) => string }): string {
  if (o.platform !== "darwin" || !o.packaged) return "";
  try {
    // Contents/Resources → Contents/Info.plist.
    return bundleVersionFromPlist(o.read(path.join(o.resourcesPath, "..", "Info.plist")));
  } catch {
    return "";
  }
}
