// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// scripts/refresh-update-info.mjs: after the release staples a disk image,
// its blockmap and its entry in the update feed describe the new bytes.
import { createHash } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterEach, describe, expect, it } from "vitest";
// @ts-expect-error: a plain .mjs script, no types
import { refresh } from "../scripts/refresh-update-info.mjs";

const dirs: string[] = [];
afterEach(() => dirs.splice(0).forEach((d) => rmSync(d, { recursive: true, force: true })));

describe("refresh-update-info", () => {
  it("rewrites the stapled disk image's sha512 and size in the feed, and its blockmap", async () => {
    const dir = mkdtempSync(path.join(tmpdir(), "refresh-"));
    dirs.push(dir);
    const dmg = path.join(dir, "Cua-Spaces-0.8.0-beta.2-universal.dmg");
    const bytes = Buffer.alloc(200_000, 7);
    writeFileSync(dmg, bytes);
    const feed = path.join(dir, "beta-mac.yml");
    writeFileSync(
      feed,
      [
        "version: 0.8.0-beta.2",
        "files:",
        "  - url: Cua-Spaces-0.8.0-beta.2-universal-mac.zip",
        "    sha512: zip",
        "    size: 10",
        "  - url: Cua-Spaces-0.8.0-beta.2-universal.dmg",
        "    sha512: old",
        "    size: 1",
        "path: Cua-Spaces-0.8.0-beta.2-universal-mac.zip",
        "sha512: zip",
        "",
      ].join("\n"),
    );
    await refresh([dmg]);
    const text = readFileSync(feed, "utf8");
    const sha = createHash("sha512").update(bytes).digest("base64");
    expect(text).toContain(`sha512: ${sha}`);
    expect(text).toContain("size: 200000");
    // The zip (what electron-updater installs on macOS) is untouched.
    expect(text).toContain("sha512: zip\n    size: 10");
    expect(readFileSync(`${dmg}.blockmap`).length).toBeGreaterThan(0);
  });

  it("fails when no feed file names the file", async () => {
    const dir = mkdtempSync(path.join(tmpdir(), "refresh-"));
    dirs.push(dir);
    const dmg = path.join(dir, "Other.dmg");
    writeFileSync(dmg, "x");
    writeFileSync(path.join(dir, "beta-mac.yml"), "files: []\n");
    await expect(refresh([dmg])).rejects.toThrow(/no update feed file/);
  });
});
