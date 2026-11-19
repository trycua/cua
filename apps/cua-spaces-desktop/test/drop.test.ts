// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The preload's part of the drop well (drop.ts): only paths the person
// dropped reach the host, whatever the page sends.
import { describe, expect, it } from "vitest";
import { withDroppedPaths } from "../src/drop";

describe("dropped paths", () => {
  it("replace whatever the page put on spaces.droppedFiles", () => {
    expect(withDroppedPaths({ id: "1", method: "spaces.droppedFiles", args: { names: ["a.txt"], paths: ["/etc/passwd"] } }, ["/Users/ada/a.txt"])).toEqual({
      id: "1",
      method: "spaces.droppedFiles",
      args: { names: ["a.txt"], paths: ["/Users/ada/a.txt"] },
    });
    expect(withDroppedPaths({ id: "2", method: "spaces.droppedFiles" }, [])).toEqual({ id: "2", method: "spaces.droppedFiles", args: { paths: [] } });
  });

  it("leave every other request as it is", () => {
    const r = { id: "3", method: "spaces.sendFiles", args: { spaceId: "x", paths: ["/a"] } };
    expect(withDroppedPaths(r, ["/b"])).toBe(r);
    expect(withDroppedPaths(null, ["/b"])).toBeNull();
  });
});
