// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Windows and Linux nightly (CUA-3503): its own feed files on its own
// release, so a Stable app (latest*.yml on cua-spaces-latest) never sees it.
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
// @ts-expect-error: plain .mjs script, no types
import { NIGHTLY_FEEDS, nightlyFeedProblems, prepareNightlyFeeds } from "../scripts/nightly-feeds.mjs";

const require = createRequire(import.meta.url);
const configPath = require.resolve("../electron-builder.config.cjs");
// The semver electron-updater compares versions with.
const semver = require(require.resolve("semver", { paths: [path.dirname(require.resolve("electron-updater"))] }));

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

const VERSION = "0.21.0-nightly.20261105.42";
const BASE = "https://github.com/trycua/cua/releases/download/cua-spaces-nightly";
const STABLE_BASE = "https://github.com/trycua/cua/releases/download/cua-spaces-latest";

const winFeed = (arch: string, version = VERSION) => `version: ${version}
files:
  - url: Cua-Spaces-Setup-${version}-${arch}.exe
    sha512: ${arch}==
    size: 100
path: Cua-Spaces-Setup-${version}-${arch}.exe
sha512: ${arch}==
releaseDate: '2026-11-05T07:30:00.000Z'
`;
const linuxFeed = (file: string, version = VERSION) => `version: ${version}
files:
  - url: ${file}
    sha512: lin==
    size: 100
path: ${file}
sha512: lin==
releaseDate: '2026-11-05T07:30:00.000Z'
`;

describe("electron-builder config: the nightly channel", () => {
  it("a nightly version publishes nightly*.yml on the cua-spaces-nightly release", () => {
    const c = config({ CUA_SPACES_VERSION: VERSION, CUA_SPACES_BUILD_NUMBER: "77" });
    expect(c.extraMetadata.version).toBe(VERSION);
    expect(c.publish).toEqual([{ provider: "generic", url: BASE, channel: "nightly" }]);
  });

  it("stable and beta keep the cua-spaces-latest feed and never the nightly channel", () => {
    expect(config({ CUA_SPACES_VERSION: "0.21.0" }).publish).toEqual([{ provider: "generic", url: STABLE_BASE, channel: "latest" }]);
    expect(config({ CUA_SPACES_VERSION: "0.21.0-beta.7" }).publish).toEqual([{ provider: "generic", url: STABLE_BASE, channel: "beta" }]);
    // Only a suffix that starts with `nightly` is the nightly.
    expect(config({ CUA_SPACES_VERSION: "0.21.0-nightlyish.1" }).publish[0].channel).toBe("beta");
  });

  it("refuses build metadata (electron-builder versions carry no +sha)", () => {
    expect(() => config({ CUA_SPACES_VERSION: `${VERSION}+55e1f5d` })).toThrow(/CUA_SPACES_VERSION/);
  });
});

describe("nightly-feeds", () => {
  const dirs: string[] = [];
  afterEach(() => {
    for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true });
  });
  function parts(files: Record<string, Record<string, string>>) {
    const root = mkdtempSync(path.join(tmpdir(), "nightly-feeds-"));
    dirs.push(root);
    const partDirs = Object.entries(files).map(([dir, contents]) => {
      const p = path.join(root, dir);
      mkdirSync(p, { recursive: true });
      for (const [name, text] of Object.entries(contents)) writeFileSync(path.join(p, name), text);
      return p;
    });
    return { root, partDirs, out: path.join(root, "out") };
  }

  it("joins the Windows feeds and points every installer at the nightly release", () => {
    const { partDirs, out } = parts({
      "part-winfeed-x64": { "nightly.yml": winFeed("x64") },
      "part-winfeed-arm64": { "nightly.yml": winFeed("arm64") },
      "part-linux-x64": { "nightly-linux.yml": linuxFeed(`Cua-Spaces-${VERSION}-x86_64.AppImage`), "x.AppImage": "" },
      "part-linux-arm64": { "nightly-linux-arm64.yml": linuxFeed(`Cua-Spaces-${VERSION}-arm64.AppImage`) },
    });
    expect(prepareNightlyFeeds({ base: BASE, version: VERSION, out, partDirs })).toEqual(NIGHTLY_FEEDS);
    const win = readFileSync(path.join(out, "nightly.yml"), "utf8");
    expect(win).toContain(`  - url: ${BASE}/Cua-Spaces-Setup-${VERSION}-x64.exe\n`);
    expect(win).toContain(`  - url: ${BASE}/Cua-Spaces-Setup-${VERSION}-arm64.exe\n`);
    expect(readFileSync(path.join(out, "nightly-linux-arm64.yml"), "utf8")).toContain(`path: ${BASE}/Cua-Spaces-${VERSION}-arm64.AppImage\n`);
  });

  it("publishes what built: a platform that failed has no feed", () => {
    const { partDirs, out } = parts({ "part-linux-x64": { "nightly-linux.yml": linuxFeed("a.AppImage") } });
    expect(prepareNightlyFeeds({ base: BASE, version: VERSION, out, partDirs })).toEqual(["nightly-linux.yml"]);
  });

  it("refuses a stable or beta feed in a nightly build", () => {
    const { partDirs, out } = parts({ "part-winfeed-x64": { "latest.yml": winFeed("x64", "0.21.0") } });
    expect(() => prepareNightlyFeeds({ base: BASE, version: VERSION, out, partDirs })).toThrow(/latest\.yml/);
    const beta = parts({ "part-linux-x64": { "beta-linux.yml": linuxFeed("a.AppImage", "0.21.0-beta.1") } });
    expect(() => prepareNightlyFeeds({ base: BASE, version: VERSION, out: beta.out, partDirs: beta.partDirs })).toThrow(/beta-linux\.yml/);
  });

  it("refuses a feed that is not a nightly, or that would land on the stable release", () => {
    const abs = (text: string, base = BASE) => text.replace(/(url|path): (\S+)/g, `$1: ${base}/$2`);
    expect(nightlyFeedProblems("nightly.yml", abs(winFeed("x64")), BASE, VERSION)).toEqual([]);
    expect(nightlyFeedProblems("nightly.yml", abs(winFeed("x64", "0.21.0")), BASE, undefined).join()).toMatch(/not X\.Y\.Z-nightly/);
    expect(nightlyFeedProblems("nightly.yml", abs(winFeed("x64", "0.21.0-beta.7")), BASE, undefined).join()).toMatch(/not X\.Y\.Z-nightly/);
    expect(nightlyFeedProblems("nightly.yml", abs(winFeed("x64"), STABLE_BASE), BASE, VERSION).join()).toMatch(/outside/);
    expect(nightlyFeedProblems("nightly.yml", abs(winFeed("x64"), STABLE_BASE), STABLE_BASE, VERSION).join()).toMatch(/not the cua-spaces-nightly release/);
    expect(nightlyFeedProblems("latest.yml", abs(winFeed("x64")), BASE, VERSION).join()).toMatch(/not a nightly feed/);
    expect(nightlyFeedProblems("nightly.yml", abs(winFeed("x64", "0.21.0-nightly.20261105.41")), BASE, VERSION).join()).toMatch(/want/);
  });

  it("every nightly is newer than the one before, even two on one day, and below the next stable", () => {
    // nightly-cua-spaces.yml: X.Y.Z-nightly.YYYYMMDD.<run number> (Electron), +sha7 on the Mac.
    const order = ["0.20.0", "0.21.0-nightly.20261105.42", "0.21.0-nightly.20261105.43", "0.21.0-nightly.20261106.44", "0.21.0"];
    for (let i = 1; i < order.length; i++) expect(semver.gt(order[i], order[i - 1])).toBe(true);
    // Run numbers compare as numbers (9 < 10), not as text.
    expect(semver.gt("0.21.0-nightly.20261105.10", "0.21.0-nightly.20261105.9")).toBe(true);
    // An HHMM time would need a leading zero, which semver (and this check) refuses.
    expect(semver.valid("0.21.0-nightly.20261105.0731")).toBeNull();
    const abs = (text: string) => text.replace(/(url|path): (\S+)/g, `$1: ${BASE}/$2`);
    expect(nightlyFeedProblems("nightly.yml", abs(winFeed("x64", "0.21.0-nightly.20261105.0731")), BASE, undefined).join()).toMatch(/not X\.Y\.Z-nightly/);
    expect(nightlyFeedProblems("nightly.yml", abs(winFeed("x64", "0.21.0-nightly.20261105")), BASE, undefined).join()).toMatch(/not X\.Y\.Z-nightly/);
  });
});
