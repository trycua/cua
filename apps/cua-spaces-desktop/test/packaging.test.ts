// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { LINUX_WINDOW_ICON, windowIconPath } from "../src/icon";

const require = createRequire(import.meta.url);
const config = require("../electron-builder.config.cjs");

const repo = path.join(import.meta.dirname, "../../..");
const readJson = (file: string) => JSON.parse(readFileSync(path.join(repo, file), "utf8"));

describe("packaging", () => {
  it("follows the Cua Spaces version (Release Please bumps both)", () => {
    const manifest = readJson(".release-please-manifest.json");
    expect(readJson("apps/cua-spaces-desktop/package.json").version).toBe(manifest["apps/cua-spaces-macos"]);
    const extras = readJson("release-please-config.json").packages["apps/cua-spaces-macos"]["extra-files"];
    expect(extras).toContainEqual({ type: "json", path: "/apps/cua-spaces-desktop/package.json", jsonpath: "$.version" });
  });

  it("installs under the product's own name, not the npm scope", () => {
    // electron-builder names the Windows install folder and the updater cache after `name`.
    expect(config.extraMetadata.name).toBe("cua-spaces");
    expect(config.extraMetadata.name).toMatch(/^[a-z0-9-]+$/);
    expect(config.productName).toBe("Cua Spaces");
  });

  it("ships the Linux window icon next to the tray icon", () => {
    const filter = config.linux.extraResources.flatMap((r: { to: string; filter: string[] }) => (r.to === "tray" ? r.filter : []));
    expect(filter).toContain(LINUX_WINDOW_ICON);
    expect(filter).toContain("32x32.png");
  });
});

describe("windowIconPath", () => {
  const base = { packaged: true, resourcesPath: "/opt/Cua Spaces/resources", devIconsDir: "/src/icons", exists: () => true };

  it("gives Linux windows the packaged icon", () => {
    expect(windowIconPath({ ...base, platform: "linux" })).toBe(path.join("/opt/Cua Spaces/resources", "tray", LINUX_WINDOW_ICON));
    expect(windowIconPath({ ...base, platform: "linux", packaged: false })).toBe(path.join("/src/icons", LINUX_WINDOW_ICON));
  });

  it("leaves Windows and macOS to the exe and the bundle, and skips a missing file", () => {
    expect(windowIconPath({ ...base, platform: "win32" })).toBeUndefined();
    expect(windowIconPath({ ...base, platform: "darwin" })).toBeUndefined();
    expect(windowIconPath({ ...base, platform: "linux", exists: () => false })).toBeUndefined();
  });
});
