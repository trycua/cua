// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The runtime the generated bindings (`generated/*-ffi.ts`) import in place
// of `@ubjs/node`: the copy-based N-API runtime that
// libs/cua/scripts/build-node-runtime.mjs builds (Electron rejects the
// upstream one's external ArrayBuffers) and the library it opens, both from
// the native directory `load.ts` located. The bindings read these when they
// load, so `useNativeFiles` runs first.
import { createRequire } from "node:module";

export interface NativeRuntimeFiles {
  /** `libcua_spaces_ffi` (both UniFFI namespaces). */
  library: string;
  /** `cua_node_runtime.node`. */
  runtime: string;
}

let files: NativeRuntimeFiles | null = null;
let nativeModule: unknown;

/** Where the library and the runtime are (before the bindings load). */
export function useNativeFiles(f: NativeRuntimeFiles): void {
  files = f;
}

function need(): NativeRuntimeFiles {
  if (!files) throw new Error("the native layer was not located: call loadNative first");
  return files;
}

const FfiType = {
  UInt8: { tag: "UInt8" },
  Int8: { tag: "Int8" },
  UInt16: { tag: "UInt16" },
  Int16: { tag: "Int16" },
  UInt32: { tag: "UInt32" },
  Int32: { tag: "Int32" },
  UInt64: { tag: "UInt64" },
  Int64: { tag: "Int64" },
  Float32: { tag: "Float32" },
  Float64: { tag: "Float64" },
  Handle: { tag: "Handle" },
  RustBuffer: { tag: "RustBuffer" },
  ForeignBytes: { tag: "ForeignBytes" },
  RustCallStatus: { tag: "RustCallStatus" },
  VoidPointer: { tag: "VoidPointer" },
  Void: { tag: "Void" },
  Callback: (name: string) => ({ tag: "Callback", name }),
  Struct: (name: string) => ({ tag: "Struct", name }),
  Reference: (inner: unknown) => ({ tag: "Reference", inner }),
  MutReference: (inner: unknown) => ({ tag: "MutReference", inner }),
};

export default {
  FfiType,
  /** Every namespace lives in the one library. */
  resolveLibPath: () => need().library,
  get UniffiNativeModule(): unknown {
    if (!nativeModule) {
      const runtime = need().runtime;
      nativeModule = (createRequire(runtime)(runtime) as { UniffiNativeModule: unknown }).UniffiNativeModule;
    }
    return nativeModule;
  },
};
