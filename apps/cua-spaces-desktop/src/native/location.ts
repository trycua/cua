// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Where the native layer lives: `cua-spaces-ffi` (the cua SDK and the Spaces
// app core in one library), its Electron-safe N-API runtime and the bundled
// `cua`. `pnpm native` builds them into `native/<platform>-<arch>/`, which
// electron-builder ships as `Resources/native/` (electron-builder.config.cjs).
// Pure, so it is tested without Electron.
import * as path from "node:path";

/** The shared library's file name on `platform`. */
export function libraryFile(platform: NodeJS.Platform): string {
  if (platform === "darwin") return "libcua_spaces_ffi.dylib";
  if (platform === "win32") return "cua_spaces_ffi.dll";
  return "libcua_spaces_ffi.so";
}

/** The N-API runtime the generated bindings call through. */
export const RUNTIME_FILE = "cua_node_runtime.node";

/** The bundled `cua` (the Spaces build of the CLI, `cua-spaces-cli`). */
export function cuaFile(platform: NodeJS.Platform): string {
  return platform === "win32" ? "cua.exe" : "cua";
}

/** The dev build's directory name for a platform and architecture. */
export const devDirName = (platform: NodeJS.Platform, arch: string) => `${platform}-${arch}`;

export interface NativeLocationInput {
  platform: NodeJS.Platform;
  arch: string;
  packaged: boolean;
  /** `process.resourcesPath`. */
  resourcesPath: string;
  /** The app directory (apps/cua-spaces-desktop) in development. */
  appRoot: string;
  /** `CUA_SPACES_NATIVE_DIR`: another build's directory (tests, a cross build). */
  override?: string;
}

/**
 * The native directory: `Resources/native` in a packaged app,
 * `native/<platform>-<arch>` in development, or `CUA_SPACES_NATIVE_DIR`
 * in either (an explicit directory always wins).
 */
export function nativeDir(o: NativeLocationInput): string {
  if (o.override) return path.resolve(o.override);
  if (o.packaged) return path.join(o.resourcesPath, "native");
  return path.join(o.appRoot, "native", devDirName(o.platform, o.arch));
}

/** The files the native layer needs, in `dir`. */
export function nativeFiles(dir: string, platform: NodeJS.Platform) {
  return {
    library: path.join(dir, libraryFile(platform)),
    runtime: path.join(dir, RUNTIME_FILE),
    cua: path.join(dir, cuaFile(platform)),
  };
}
