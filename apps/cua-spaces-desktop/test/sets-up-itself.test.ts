// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Drift guard: the runtimes that count as ready while still "installable"
// (cua installs them on the first create that needs them) are the same in
// this app and the SwiftUI app, and include Firecracker (microVMs on Linux
// install it on first use).
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const read = (rel: string) => readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
const names = (s: string) => [...s.matchAll(/"([a-z]+)"/g)].map((m) => m[1]).sort();

describe("runtimes cua sets up by itself", () => {
  it("are the same in the Electron and SwiftUI apps, Firecracker included", () => {
    const electron = /const setsUpItself = new Set\(\[([^\]]*)\]\)/.exec(read("../src/model/backend.ts"))?.[1];
    const swift = /let setsUpItself: Set<String> = \[([^\]]*)\]/.exec(read("../../cua-spaces-macos/Sources/CuaSpacesMacKit/Models/SpacesBackend.swift"))?.[1];
    expect(electron).toBeDefined();
    expect(swift).toBeDefined();
    expect(names(electron!)).toEqual(names(swift!));
    expect(names(electron!)).toContain("firecracker");
  });
});
