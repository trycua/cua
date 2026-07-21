# Experimental cua-driver TypeScript client

This private reference package is generated from the canonical cua-driver
contract and speaks MCP over a spawned `cua-driver mcp` stdio process.

```ts
import { CuaDriverClient } from "@trycua/cua-driver-client"

const client = CuaDriverClient.stdio()
try {
  const result = await client.startSession({ session: "demo", captureScope: "auto" })
  const desktop = await client.getDesktopState({ session: "demo" })
  await client.click({ x: 420, y: 240, scope: "desktop", session: "demo" })
  console.log(desktop.images[0]?.mimeType)
} finally {
  await client.close()
}
```

The native driver remains the execution, policy, and approval boundary. This
experimental package must not be published from the draft branch.
Requests have bounded timeouts, and action calls are never retried automatically.
