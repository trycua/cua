# cua-driver TypeScript SDK

Rust-backed TypeScript/Node SDK for Cua Driver client applications.

## Product boundary

This package has one public entrypoint:

```ts
import { CuaDriver } from "@trycua/cua-driver"
```

It does not contain a TypeScript MCP client. Agents already have
runtime-neutral MCP clients and should configure the executable directly:

```text
cua-driver mcp
```

The removed pre-release MCP facade used `CuaDriver.stdio()`, async methods,
`*Args` interfaces, and a TypeScript stdio transport. Application code migrates
to the synchronous Rust-backed methods shown below; agent code removes the Cua
package import and supplies `cua-driver mcp` to its agent SDK.

## SDK example

```ts
import {
  CaptureScope,
  CuaDriver,
  EndSessionInput,
  GetDesktopStateInput,
  StartSessionInput,
} from "@trycua/cua-driver"

const driver = CuaDriver.connect(undefined) // default installed daemon socket
driver.startSession(
  StartSessionInput.new({
    session: "demo",
    captureScope: CaptureScope.Desktop,
  }),
)

try {
  const desktop = driver.getDesktopState(
    GetDesktopStateInput.new({ session: "demo" }),
  )
  console.log(desktop.images[0]?.mimeType)
} finally {
  driver.endSession(EndSessionInput.new({ session: "demo" }))
  driver.uniffiDestroy()
}
```

The SDK is currently synchronous and requires a native library matching the
host OS and architecture. Desktop calls return a typed `ToolResult` with text,
images, verification/error metadata, and `structuredJson` / `rawJson` for
platform-extensible results. Session lifecycle calls return dedicated
generated records.

Host-native assembly and loader tests are implemented. Publishing the npm
package still requires assembling and testing the complete platform-library
matrix rather than packing a developer's local library.
