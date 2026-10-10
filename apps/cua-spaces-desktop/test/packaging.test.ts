// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, symlinkSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
import { LINUX_WINDOW_ICON, windowIconPath } from "../src/icon";

const require = createRequire(import.meta.url);
const config = require("../electron-builder.config.cjs");
const { normalizeModes } = require("../packaging/linux-permissions.cjs");

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

describe("the Linux install tree", () => {
  const dirs: string[] = [];
  afterEach(() => dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));
  const mode = (p: string) => statSync(p).mode & 0o7777;

  /** An unpacked app as a build machine with umask 002 leaves it. */
  function groupWritableApp(): string {
    const root = mkdtempSync(path.join(tmpdir(), "cua-linux-unpacked-"));
    dirs.push(root);
    const native = path.join(root, "resources", "native");
    mkdirSync(native, { recursive: true });
    const files = { "cua-spaces": 0o775, "resources.pak": 0o664, "resources/native/cua": 0o775, "resources/native/libcua_spaces_ffi.so": 0o664, "resources/native/cua_node_runtime.node": 0o664 };
    for (const [f, m] of Object.entries(files)) {
      writeFileSync(path.join(root, f), "x");
      chmodSync(path.join(root, f), m);
    }
    for (const d of [root, path.join(root, "resources"), native]) chmodSync(d, 0o775);
    symlinkSync("cua-spaces", path.join(root, "link"));
    return root;
  }

  it("is writable by its owner only, executables kept executable", () => {
    const root = groupWritableApp();
    normalizeModes(root);
    // What the Keyvault asks of every folder above the bundled cua (cua-keyvault `root_protected`).
    for (const d of [root, path.join(root, "resources"), path.join(root, "resources/native")]) expect(mode(d)).toBe(0o755);
    expect(mode(path.join(root, "resources/native/cua"))).toBe(0o755);
    expect(mode(path.join(root, "cua-spaces"))).toBe(0o755);
    expect(mode(path.join(root, "resources/native/libcua_spaces_ffi.so"))).toBe(0o644);
    expect(mode(path.join(root, "resources.pak"))).toBe(0o644);
  });

  it("is fixed by afterPack before the deb and the AppImage are made", async () => {
    const root = groupWritableApp();
    await config.afterPack({ electronPlatformName: "linux", appOutDir: root, packager: { appInfo: { productFilename: "Cua Spaces" } } });
    expect(mode(path.join(root, "resources/native"))).toBe(0o755);
    expect(mode(path.join(root, "resources/native/cua"))).toBe(0o755);
  });

  it("describes the app in the desktop entry and the package, not the shell it is built with", () => {
    expect(config.linux.description).toBe("Run apps and agents in Cua Spaces");
    // electron-builder writes the description over an entry's own Comment.
    expect(config.linux.desktop.entry.Comment).toBeUndefined();
    expect(config.linux.description).not.toMatch(/electron/i);
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
