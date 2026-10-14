// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Where the main process finds the native layer, and what it refuses.
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { createRequire } from "node:module";
import { describe, expect, it } from "vitest";
import { cuaFile, devDirName, libraryFile, nativeDir, nativeFiles, RUNTIME_FILE } from "../src/native/location";
import { loadNative, NativeLoadError } from "../src/native/load";

const base = { arch: "arm64", resourcesPath: "/Applications/Cua Spaces.app/Contents/Resources", appRoot: "/src/apps/cua-spaces-desktop" };

describe("nativeDir", () => {
  it("is Resources/native in a packaged app", () => {
    expect(nativeDir({ ...base, platform: "darwin", packaged: true })).toBe(path.join(base.resourcesPath, "native"));
  });

  it("is native/<platform>-<arch> in development", () => {
    expect(nativeDir({ ...base, platform: "darwin", packaged: false })).toBe(path.join(base.appRoot, "native", "darwin-arm64"));
    expect(nativeDir({ ...base, platform: "win32", arch: "x64", packaged: false })).toBe(path.join(base.appRoot, "native", "win32-x64"));
    expect(devDirName("linux", "arm64")).toBe("linux-arm64");
  });

  it("takes CUA_SPACES_NATIVE_DIR over both", () => {
    expect(nativeDir({ ...base, platform: "linux", packaged: true, override: "/opt/native" })).toBe(path.resolve("/opt/native"));
  });
});

describe("nativeFiles", () => {
  it("names each platform's library, the runtime and the bundled cua", () => {
    expect(libraryFile("darwin")).toBe("libcua_spaces_ffi.dylib");
    expect(libraryFile("win32")).toBe("cua_spaces_ffi.dll");
    expect(libraryFile("linux")).toBe("libcua_spaces_ffi.so");
    expect(cuaFile("win32")).toBe("cua.exe");
    expect(cuaFile("linux")).toBe("cua");
    expect(nativeFiles("/n", "linux")).toEqual({
      library: path.join("/n", "libcua_spaces_ffi.so"),
      runtime: path.join("/n", RUNTIME_FILE),
      cua: path.join("/n", "cua"),
    });
  });
});

describe("loadNative", () => {
  it("refuses a directory without the library, naming the file", async () => {
    const dir = mkdtempSync(path.join(tmpdir(), "cua-native-missing-"));
    try {
      await expect(loadNative(dir, "linux")).rejects.toThrow(NativeLoadError);
      await expect(loadNative(dir, "linux")).rejects.toThrow(path.join(dir, "libcua_spaces_ffi.so"));
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});

describe("packaging", () => {
  const config = createRequire(import.meta.url)("../electron-builder.config.cjs");

  it("ships each platform's native directory of the arch being packed as Resources/native", () => {
    expect(config.mac.extraResources).toContainEqual({ from: "native/darwin-${arch}", to: "native", filter: ["**/*"] });
    expect(config.win.extraResources).toContainEqual({ from: "native/win32-${arch}", to: "native", filter: ["**/*"] });
    expect(config.linux.extraResources).toContainEqual({ from: "native/linux-${arch}", to: "native", filter: ["**/*"] });
    expect(typeof config.afterPack).toBe("function");
  });
});

describe("the bindings load lazily", () => {
  it("only load.ts imports them as values; everything else takes their types", async () => {
    const { readdirSync, readFileSync } = await import("node:fs");
    const src = path.resolve(__dirname, "../src");
    const files = (readdirSync(src, { recursive: true }) as string[]).filter((f) => f.endsWith(".ts") && !f.startsWith(`native${path.sep}generated`));
    const eager = files.filter((f) => {
      if (f === path.join("native", "load.ts")) return false;
      const text = readFileSync(path.join(src, f), "utf8");
      return [...text.matchAll(/^import (?!type )[^;]*from "[./]*(native\/)?generated\/index"/gm)].length > 0;
    });
    expect(eager).toEqual([]);
  });
});
