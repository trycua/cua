// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// "<Space> is ready": the note a create from the first run posts (the SwiftUI
// host's `SpaceReadyNote`) and the image it carries.
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import { SPACE_READY_ICON, spaceReadyNote } from "../src/bridge/space-ready";
import { noteIconPath } from "../src/note-icon";

describe("the Space ready note", () => {
  it("says what is ready, names the OS, and opens the Space's page", () => {
    expect(spaceReadyNote({ id: "local:aurora", name: "aurora", os: "linux" }, "darwin")).toEqual({
      id: "space-ready-local:aurora",
      title: "aurora is ready",
      body: "A fresh Linux computer inside this Mac. Your agent can use it now.",
      route: "/spaces/local%3Aaurora",
      icon: "cua-key-128.png",
    });
    expect(spaceReadyNote({ id: "a", name: "a", os: "macos" }, "darwin").body).toMatch(/^A fresh macOS computer/);
    expect(spaceReadyNote({ id: "a", name: "a", os: "windows" }, "darwin").body).toMatch(/^A fresh Windows computer/);
    expect(spaceReadyNote({ id: "a", name: "a", os: "unknown" }, "darwin").body).toBe("A fresh computer inside this Mac. Your agent can use it now.");
  });

  it("says this machine where the app does not run on a Mac", () => {
    for (const platform of ["win32", "linux"] as const) {
      expect(spaceReadyNote({ id: "a", name: "a", os: "linux" }, platform).body).toBe("A fresh Linux computer inside this machine. Your agent can use it now.");
    }
  });

  it("carries the Cua key from the web build", () => {
    expect(SPACE_READY_ICON).toBe("cua-key-128.png");
  });
});

describe("a note's image", () => {
  const root = mkdtempSync(path.join(tmpdir(), "cua-note-icon-"));
  writeFileSync(path.join(root, "cua-key-128.png"), "png");
  writeFileSync(path.join(path.dirname(root), "outside.png"), "png");
  afterAll(() => {
    rmSync(root, { recursive: true, force: true });
    rmSync(path.join(path.dirname(root), "outside.png"), { force: true });
  });

  it("is a file of the web build, by its plain name", () => {
    expect(noteIconPath(root, "cua-key-128.png")).toBe(path.join(root, "cua-key-128.png"));
  });

  it("is none when the build lacks it or the name is a path", () => {
    expect(noteIconPath(root, "missing.png")).toBeNull();
    expect(noteIconPath(root, "../outside.png")).toBeNull();
    expect(noteIconPath(root, path.join(root, "cua-key-128.png"))).toBeNull();
    expect(noteIconPath(root, "")).toBeNull();
    expect(noteIconPath(root, ".hidden")).toBeNull();
  });
});
