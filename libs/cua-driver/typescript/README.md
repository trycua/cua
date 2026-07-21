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
