// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The Windows x64 and arm64 builds are packed in separate jobs; their feeds are
// joined into the one latest.yml / beta.yml the updater reads.
import * as path from "node:path";
import { pathToFileURL } from "node:url";
import { describe, expect, it } from "vitest";

const feed = (arch: string, sha: string) => `version: 0.9.0
files:
  - url: Cua-Spaces-Setup-0.9.0-${arch}.exe
    sha512: ${sha}
    size: 100
path: Cua-Spaces-Setup-0.9.0-${arch}.exe
sha512: ${sha}
releaseDate: '2026-10-14T00:00:00.000Z'
`;

describe("merge-feeds", () => {
  it("keeps the first feed's top level and lists every installer", async () => {
    const { mergeFeeds } = await import(pathToFileURL(path.join(import.meta.dirname, "../scripts/merge-feeds.mjs")).href);
    const merged = mergeFeeds([feed("x64", "AAA"), feed("arm64", "BBB")]);
    expect(merged).toContain("  - url: Cua-Spaces-Setup-0.9.0-x64.exe\n    sha512: AAA\n    size: 100\n");
    expect(merged).toContain("  - url: Cua-Spaces-Setup-0.9.0-arm64.exe\n    sha512: BBB\n    size: 100\n");
    expect(merged).toContain("path: Cua-Spaces-Setup-0.9.0-x64.exe\nsha512: AAA\n");
    expect(merged.match(/- url:/g)).toHaveLength(2);
  });

  it("lists an installer once and refuses a feed without files", async () => {
    const { mergeFeeds } = await import(pathToFileURL(path.join(import.meta.dirname, "../scripts/merge-feeds.mjs")).href);
    expect(mergeFeeds([feed("x64", "AAA"), feed("x64", "AAA")]).match(/- url:/g)).toHaveLength(1);
    expect(() => mergeFeeds(["version: 1\n"])).toThrow(/no files/);
  });
});
