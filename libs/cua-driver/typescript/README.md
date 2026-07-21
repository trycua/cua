# cua-driver TypeScript SDK

This package is generated from the canonical cua-driver contract and speaks MCP
over a spawned `cua-driver mcp` stdio process.

```ts
import { CuaDriver } from "@trycua/cua-driver"

const driver = CuaDriver.stdio()
try {
  const result = await driver.startSession({ session: "demo", captureScope: "auto" })
  const desktop = await driver.getDesktopState({ session: "demo" })
  await driver.click({ x: 420, y: 240, scope: "desktop", session: "demo" })
  console.log(desktop.images[0]?.mimeType)
} finally {
  await driver.close()
}
```

The native driver remains the execution, policy, and approval boundary.
Requests have bounded timeouts, and action calls are never retried automatically.

## Experimental native SDK

The opt-in `/native` subpath is generated from the Rust UniFFI interface and
calls the daemon through the shared Rust socket protocol. The package root does
not load a native library, so existing MCP-only consumers remain portable:

```ts
import { CuaDriver, GetDesktopStateInput } from "@trycua/cua-driver/native"

const driver = CuaDriver.connect(undefined)
const desktop = driver.getDesktopState(
  GetDesktopStateInput.new({ session: "demo" }),
)
console.log(desktop.images[0]?.mimeType)
driver.uniffiDestroy()
```

The package must contain a native library matching the host OS and architecture,
and the daemon must already be running. Host-native assembly and loader tests
are implemented; publishing the npm package requires building the complete
platform library matrix rather than packing a developer's local library.
