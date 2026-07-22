# cua-driver TypeScript SDK

Rust-backed TypeScript/Node SDK for Cua Driver client applications.

## Product boundary

The package root exposes the native daemon SDK:

```ts
import { CuaDriver } from "@trycua/cua-driver"
```

The `/embedded` and `/electron` entrypoints let a signed desktop host own a
private daemon and its macOS permission onboarding. The package does not contain
a TypeScript MCP client. Agents already have
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

## Embedded Node and Electron hosts

A signed desktop application can bundle `cua-driver`, start it as a direct
child, and connect both the native SDK and its agent runtime to the same private
daemon:

```ts
import { CuaDriver } from "@trycua/cua-driver"
import { EmbeddedCuaDriver } from "@trycua/cua-driver/embedded"

const embedded = new EmbeddedCuaDriver({
  binaryPath: "/path/inside/YourApp.app/cua-driver",
  hostBundleId: "com.example.your-app",
})

try {
  const connection = await embedded.start()
  const driver = CuaDriver.connect(connection.socketPath)
  // Application calls use driver; an agent runtime uses connection.mcp.
} finally {
  await embedded.stop()
}
```

On macOS, the application must spawn the daemon from the process that owns the
Accessibility and Screen Recording grants. Launching through a gateway,
terminal, `open`, or `NSWorkspace` changes the responsibility chain.

Electron main processes can request the grants after `app.whenReady()`:

```ts
import {
  hasRequiredMacOSPermissions,
  openMacOSScreenRecordingSettings,
  requestMacOSPermissions,
} from "@trycua/cua-driver/electron"

const permissions = requestMacOSPermissions()
if (!hasRequiredMacOSPermissions(permissions) && !permissions.screenRecording) {
  await openMacOSScreenRecordingSettings()
}
```

Stop the daemon before the host exits. If grants change while it is running,
restart it so macOS evaluates them in a fresh process. The package does not
install or bundle the executable; keep it and the Electron adapter's `ffi-rs`
native module outside ASAR and sign the nested executable before the host app.

Host-native assembly and loader tests are implemented. Publishing the npm
package still requires assembling and testing the complete platform-library
matrix rather than packing a developer's local library.
