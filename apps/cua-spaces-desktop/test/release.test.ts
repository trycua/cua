// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import * as path from "node:path";
import { describe, expect, it, vi } from "vitest";
// @ts-expect-error: plain .mjs script, no types
import { absolutize } from "../scripts/feed-urls.mjs";

vi.mock("electron", () => ({ app: { getVersion: () => "0.0.0", isPackaged: false }, BrowserWindow: { getAllWindows: () => [] } }));
const { buildChannel, channelServed, feedChannel, STABLE_FEED } = await import("../src/updater");

const require = createRequire(import.meta.url);
const configPath = require.resolve("../electron-builder.config.cjs");
const swiftPlist = readFileSync(path.join(import.meta.dirname, "../../cua-spaces-macos/Support/Info.plist"), "utf8");

/** The electron-builder config as CI would load it with `env`. */
function config(env: Record<string, string | undefined> = {}) {
  const saved = { ...process.env };
  for (const k of ["CUA_SPACES_VERSION", "CUA_SPACES_BUILD_NUMBER", "GITHUB_REPOSITORY"]) delete process.env[k];
  Object.assign(process.env, env);
  delete require.cache[configPath];
  try {
    return require(configPath);
  } finally {
    process.env = saved;
    delete require.cache[configPath];
  }
}

const swiftString = (key: string) => new RegExp(`<key>${key}</key>\\s*<string>([^<]*)</string>`).exec(swiftPlist)?.[1];

describe("release config", () => {
  it("is the Swift app on macOS and keeps its own id elsewhere", () => {
    const c = config();
    expect(c.mac.appId).toBe("com.trycua.spaces.macos");
    expect(swiftString("CFBundleIdentifier")).toBe(c.mac.appId);
    expect(c.appId).toBe("ai.cua.spaces.desktop");
    expect(c.productName).toBe("Cua Spaces");
  });

  it("carries the Swift app's usage descriptions and Sparkle key", () => {
    const info = config().mac.extendInfo;
    for (const key of ["NSAppleEventsUsageDescription", "NSLocalNetworkUsageDescription", "SUPublicEDKey"]) {
      expect(info[key], key).toBe(swiftString(key));
      expect(info[key], key).toBeTruthy();
    }
    expect(info.SUFeedURL).toBeUndefined();
  });

  it("grants Apple Events to the app, not to its helpers", () => {
    const read = (f: string) => readFileSync(path.join(import.meta.dirname, "..", f), "utf8");
    const c = config();
    expect(read(c.mac.entitlements)).toContain("com.apple.security.automation.apple-events");
    expect(read(c.mac.entitlementsInherit)).not.toContain("automation.apple-events");
    expect(read(c.mac.entitlementsInherit)).toContain("com.apple.security.cs.allow-jit");
  });

  it("stable: package.json's version on the latest channel, feed on trycua/cua by default", () => {
    const c = config();
    const pkg = JSON.parse(readFileSync(path.join(import.meta.dirname, "../package.json"), "utf8"));
    expect(c.extraMetadata.version).toBe(pkg.version);
    expect(c.buildVersion).toBe(`${pkg.version}.0`);
    expect(c.publish).toEqual([
      { provider: "generic", url: "https://github.com/trycua/cua/releases/download/cua-spaces-latest", channel: "latest" },
    ]);
  });

  it("prerelease: the tag's version on the beta channel, feed on the building repository", () => {
    const c = config({ CUA_SPACES_VERSION: "0.8.0-beta.1", CUA_SPACES_BUILD_NUMBER: "412", GITHUB_REPOSITORY: "example/cua-fork" });
    expect(c.extraMetadata.version).toBe("0.8.0-beta.1");
    // The Swift app's CFBundleVersion scheme: X.Y.Z.<run number>.
    expect(c.buildVersion).toBe("0.8.0.412");
    expect(c.publish).toEqual([
      { provider: "generic", url: "https://github.com/example/cua-fork/releases/download/cua-spaces-latest", channel: "beta" },
    ]);
  });

  it("refuses a malformed version, build number or repository", () => {
    expect(() => config({ CUA_SPACES_VERSION: "v0.8.0" })).toThrow(/CUA_SPACES_VERSION/);
    expect(() => config({ CUA_SPACES_BUILD_NUMBER: "12a" })).toThrow(/CUA_SPACES_BUILD_NUMBER/);
    expect(() => config({ GITHUB_REPOSITORY: "https://github.com/x/y" })).toThrow(/GITHUB_REPOSITORY/);
  });
});

describe("updater channels", () => {
  it("a suffixed version is a beta", () => {
    expect(buildChannel("0.8.0-beta.1")).toBe("beta");
    expect(buildChannel("0.8.0-staging.3")).toBe("beta");
    expect(buildChannel("0.8.0")).toBe("stable");
    expect(feedChannel("beta")).toBe("beta");
    expect(feedChannel("stable")).toBe("latest");
  });

  it("serves only the beta channel until the cutover", () => {
    expect(STABLE_FEED).toBe(false);
    expect(channelServed("beta")).toBe(true);
    expect(channelServed("stable")).toBe(false);
    expect(channelServed("stable", true)).toBe(true);
  });
});

describe("feed-urls", () => {
  const feed = [
    "version: 0.8.0-beta.1",
    "files:",
    "  - url: Cua-Spaces-0.8.0-beta.1-arm64-mac.zip",
    "    sha512: abc==",
    "    size: 10",
    "  - url: https://example.com/already.zip",
    "    sha512: def==",
    "path: Cua-Spaces-0.8.0-beta.1-arm64-mac.zip",
    "sha512: abc==",
    "releaseDate: '2026-10-07T00:00:00.000Z'",
    "",
  ].join("\n");
  const base = "https://github.com/trycua/cua/releases/download/cua-spaces-v0.8.0-beta.1";

  it("puts relative installer URLs under the version's release", () => {
    const out = absolutize(feed, base);
    expect(out).toContain(`  - url: ${base}/Cua-Spaces-0.8.0-beta.1-arm64-mac.zip\n`);
    expect(out).toContain(`path: ${base}/Cua-Spaces-0.8.0-beta.1-arm64-mac.zip\n`);
    expect(out).toContain("  - url: https://example.com/already.zip\n");
    expect(out).toContain("sha512: abc==\n");
    expect(out).toContain("releaseDate: '2026-10-07T00:00:00.000Z'");
  });

  it("keeps quotes and escapes odd names", () => {
    expect(absolutize("path: 'Cua Spaces 1.zip'\n", `${base}/`)).toBe(`path: '${base}/Cua%20Spaces%201.zip'\n`);
    expect(() => absolutize(feed, "http://insecure")).toThrow(/https/);
  });
});
