import { createRequire } from "node:module"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

import { resolveLibPath } from "@ubjs/node/typescript/dist/resolve-lib.js"

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
}

const sdkLibrary = resolveLibPath({
  crateName: "cua_driver_sdk",
  callerUrl: import.meta.url,
  npmPackageBase: "@trycua/cua-driver-",
  tripleStyle: "node",
})
const runtimePath = join(dirname(sdkLibrary), "cua_driver_node_runtime.node")
const require = createRequire(import.meta.url)
const { UniffiNativeModule } = require(runtimePath) as {
  UniffiNativeModule: unknown
}

export default { FfiType, resolveLibPath, UniffiNativeModule }
