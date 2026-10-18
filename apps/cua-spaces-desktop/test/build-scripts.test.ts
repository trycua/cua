// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The build scripts run on macOS, Linux and Windows (cmd.exe), and a local
// package build packs only the arches it has a native layer for.
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import * as path from "node:path";
import { pathToFileURL } from "node:url";
import { describe, expect, it } from "vitest";

const require = createRequire(import.meta.url);
const pkg = require("../package.json");
const { packageArchs } = require("../packaging/archs.cjs");
const root = path.join(import.meta.dirname, "..");

describe("the scripts on Windows", () => {
  it("use no POSIX-only commands cmd.exe lacks", () => {
    for (const name of ["build", "dist:win", "native", "test", "typecheck"]) {
      expect(pkg.scripts[name], name).not.toMatch(/\brm -|\bcp -|\bmkdir -p\b/);
    }
  });

  it("run npm as npm.cmd, through the shell, with arguments quoted", async () => {
    const source = readFileSync(path.join(root, "scripts/build-native.mjs"), "utf8");
    expect(source).toContain('process.platform === "win32" ? "npm.cmd" : "npm"');
    const script = pathToFileURL(path.join(root, "scripts/build-native.mjs")).href;
    const { shellCommand } = (await import(/* @vite-ignore */ script)) as { shellCommand: (command: string, args: string[]) => string };
    expect(shellCommand("npm.cmd", ["ci", "--ignore-scripts"])).toBe("npm.cmd ci --ignore-scripts");
    expect(shellCommand("C:\\a b\\ubrn.cmd", ["--library", "C:\\x y\\lib.dll"])).toBe('"C:\\a b\\ubrn.cmd" --library "C:\\x y\\lib.dll"');
  });

  it("lists CMake among the Windows prerequisites for a local build", () => {
    const readme = readFileSync(path.join(root, "README.md"), "utf8");
    expect(readme).toMatch(/CMake/);
  });
});

describe("the arches a package is made for", () => {
  const folders = (...dirs: string[]) => (dir: string) => dirs.includes(dir);

  it("are all of them in a release, which refuses any without its native layer", () => {
    expect(packageArchs("linux", ["x64", "arm64"], undefined, folders())).toEqual(["x64", "arm64"]);
    expect(packageArchs("linux", ["x64", "arm64"], "", folders())).toEqual(["x64", "arm64"]);
  });

  it("are those with a native folder when CUA_SPACES_ARCHS=native", () => {
    expect(packageArchs("linux", ["x64", "arm64"], "native", folders("linux-x64"))).toEqual(["x64"]);
    expect(packageArchs("win32", ["x64", "arm64"], "native", folders("win32-arm64", "linux-x64"))).toEqual(["arm64"]);
    // A universal mac build merges both darwin folders.
    expect(packageArchs("darwin", ["arm64", "universal"], "native", folders("darwin-arm64"))).toEqual(["arm64"]);
    expect(packageArchs("darwin", ["arm64", "universal"], "native", folders("darwin-arm64", "darwin-x64"))).toEqual(["arm64", "universal"]);
  });

  it("are the listed ones, and all of them when the setting leaves none (the build says which layer is missing)", () => {
    expect(packageArchs("linux", ["x64", "arm64"], "x64", folders())).toEqual(["x64"]);
    expect(packageArchs("linux", ["x64", "arm64"], " arm64 , x64 ", folders())).toEqual(["x64", "arm64"]);
    expect(packageArchs("linux", ["x64", "arm64"], "native", folders())).toEqual(["x64", "arm64"]);
    expect(packageArchs("linux", ["x64", "arm64"], "riscv64", folders())).toEqual(["x64", "arm64"]);
  });
});
