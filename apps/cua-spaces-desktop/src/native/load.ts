// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Loads the native layer into this process: the generated bindings
// (`generated/`, both UniFFI namespaces of `cua-spaces-ffi`) over the
// Electron-safe N-API runtime, from the directory `location.ts` names. Then
// registers the Cua Spaces extensions, as the SwiftUI app does at launch
// before its first `Cua`.
import { existsSync } from "node:fs";
import { nativeFiles } from "./location";
import { useNativeFiles } from "./node-runtime";

export type Native = typeof import("./generated/index");

/** The native layer is not where it should be (or would not load). */
export class NativeLoadError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "NativeLoadError";
  }
}

let loading: { dir: string; native: Promise<Native> } | null = null;

/** The bindings, loaded once per process from `dir`. */
export function loadNative(dir: string, platform: NodeJS.Platform = process.platform): Promise<Native> {
  if (loading) {
    if (loading.dir !== dir) return Promise.reject(new NativeLoadError(`the native layer is already loaded from ${loading.dir}`));
    return loading.native;
  }
  const native = (async () => {
    const files = nativeFiles(dir, platform);
    for (const file of [files.library, files.runtime]) {
      if (!existsSync(file)) throw new NativeLoadError(`missing ${file}`);
    }
    useNativeFiles(files);
    let n: Native;
    try {
      n = await import("./generated/index");
    } catch (error) {
      throw new NativeLoadError(`could not load ${files.library}: ${error instanceof Error ? error.message : String(error)}`);
    }
    n.cuaSpacesRegister();
    return n;
  })();
  loading = { dir, native };
  native.catch(() => {
    if (loading?.native === native) loading = null;
  });
  return native;
}
