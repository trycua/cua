// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import { diskBytes, movableKinds, tildePath, userHomeOf } from "./disk-storage";

describe("disk storage words", () => {
  it("writes sizes the way Finder does", () => {
    expect(diskBytes(11_300_000_000)).toBe("11.3 GB");
    expect(diskBytes(512_000_000)).toBe("512 MB");
    expect(diskBytes(0)).toBe("0 KB");
    expect(diskBytes(1_500_000_000_000)).toBe("1.5 TB");
  });

  it("shows paths under the home folder with a tilde", () => {
    expect(tildePath("/Users/maya/.cua/cache", "/Users/maya")).toBe("~/.cua/cache");
    expect(tildePath("/Volumes/Fast/Cua", "/Users/maya")).toBe("/Volumes/Fast/Cua");
    expect(tildePath("/Users/mayaq/x", "/Users/maya")).toBe("/Users/mayaq/x");
    expect(tildePath("/Users/maya", "/Users/maya/")).toBe("~");
  });

  it("finds the user's home from the cua home of either channel", () => {
    expect(userHomeOf("/Users/maya/.cua")).toBe("/Users/maya");
    expect(userHomeOf("/Users/maya/.cua-local")).toBe("/Users/maya");
    expect(userHomeOf("/Users/maya/.cua/envs/staging")).toBe("/Users/maya");
    expect(userHomeOf("/data/cua-home")).toBeNull();
  });

  it("lists only the folders that can move, in a fixed order", () => {
    const k = (kind: string, movable = true) => ({ kind, path: "/p", source: "default", movable, bytes: 1 });
    const got = movableKinds([k("config", false), k("logs"), k("runtimes"), k("keys", false), k("trajectories")]);
    expect(got.map((x) => x.kind)).toEqual(["runtimes", "logs", "trajectories"]);
  });
});
