# cua-driver TypeScript SDK

Rust-backed TypeScript/Node SDK for Cua Driver client applications.

## Product boundary

The package root exposes the native SDK:

```ts
import { CuaDriver } from "@trycua/cua-driver"
```

`EmbeddedCuaDriverHost` is exported from both the package root and the
organizational `/embedded` entrypoint; they are the same generated Rust object.
The `/electron` entrypoint is a thin compatibility naming layer over the same
generated macOS permission functions. The package does not contain a TypeScript
MCP client. Agents already have
runtime-neutral MCP clients and should configure the executable directly:

```text
cua-driver mcp
```

The removed pre-release MCP facade used `CuaDriver.stdio()`, `*Args`
interfaces, and a TypeScript stdio transport. Application code imports the
typed Rust-backed SDK shown below; agent code supplies `cua-driver mcp` to its
agent SDK.

## SDK example

```ts
import {
  CaptureScope,
  CuaDriver,
  EndSessionInput,
  GetDesktopStateInput,
  StartSessionInput,
} from "@trycua/cua-driver"

const driver = CuaDriver.create(undefined) // same process; no daemon
await driver.startSession(
  StartSessionInput.new({
    session: "demo",
    captureScope: CaptureScope.Desktop,
  }),
)

try {
  const desktop = await driver.getDesktopState(
    GetDesktopStateInput.new({ session: "demo" }),
  )
  console.log(desktop.images[0]?.mimeType)
} finally {
  await driver.endSession(EndSessionInput.new({ session: "demo" }))
  await driver.shutdown()
  driver.uniffiDestroy()
}
```

SDK operations are asynchronous and require a native library matching the host
OS and architecture. Desktop calls return a typed `ToolResult` with text,
images, verification/error metadata, and `structuredJson` / `rawJson` for
platform-extensible results. Session lifecycle calls return dedicated generated
records.

`CuaDriver.connect(socketPath)` remains available while existing applications
migrate. It exposes the same methods over the installed daemon, but it does not
provide a second SDK contract.

`shutdown()` closes admission, waits for already admitted operations to finish,
and is idempotent. Calls started after shutdown reject with `DriverError`.
`uniffiDestroy()` releases the binding handle, but orderly applications should
await `shutdown()` first.

## Daemon-backed MCP hosts

A signed desktop application that must also expose MCP to an external agent can
bundle `cua-driver`, start it as a direct child, and connect both application
code and its agent runtime to the same private daemon:

```ts
import { CuaDriver, EmbeddedCuaDriverHost } from "@trycua/cua-driver"

const embedded = new EmbeddedCuaDriverHost(
  "/path/inside/YourApp.app/Contents/Resources/cua-driver",
  "com.example.your-app",
)

try {
  const connection = await embedded.start()
  const driver = CuaDriver.connect(connection.socketPath)
  try {
    // Application calls use driver; an agent runtime uses connection.mcp.
  } finally {
    driver.uniffiDestroy()
  }
} finally {
  await embedded.stop()
  embedded.uniffiDestroy()
}
```

On macOS, the application must spawn the daemon from the process that owns the
Accessibility and Screen Recording grants. Launching through a gateway,
terminal, `open`, or `NSWorkspace` changes the responsibility chain.

Electron main processes can call the permission primitives after
`app.whenReady()`. These functions run in the importing host process, so macOS
attributes their requests to the host rather than to the npm package or child
driver:

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

The adapter does not provide dialogs, settings rows, or onboarding policy. Do
not start the daemon until `hasRequiredMacOSPermissions()` returns true. Stop the
daemon before the host exits. If grants change while it is running, destroy all
SDK clients and MCP proxies, call `embedded.restart()`, and reconnect using the
new connection: every restart changes the generation, PID, and endpoint.

`start()` is concurrency-safe and coalesces callers. `stop()` cancels an
in-progress start and is idempotent. Treat a returned connection as valid only
for its generation, observe unexpected termination with
`waitForExit(connection.generation)`, and stop new work before teardown. The
Rust host also closes a parent-liveness pipe on normal destruction so the daemon
cannot remain orphaned after a host crash.

The npm package installs one optional native package selected for the current
OS and CPU. It does not bundle the `cua-driver` executable: ship that executable
outside ASAR, preserve its executable bit, and sign it before signing and
notarizing the enclosing app.

Each native package also carries Cua's copy-mode build of the pinned
`@ubjs/node` N-API runtime. Upstream `0.31.0-3` returns Rust-owned memory through
external ArrayBuffers, which Electron 20 and newer intentionally reject because
of V8's memory cage. The copy-mode runtime keeps the generated UniFFI API and
RustBuffer ownership contract unchanged, but copies at the native/JavaScript
boundary before freeing Rust-owned return buffers. Regular Node and Electron
therefore receive the same values; Electron hosts do not need to spawn or
configure the private daemon themselves.
