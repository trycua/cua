/**
 * Rust-backed SDK for Cua Driver client applications.
 *
 * Agents should configure `cua-driver mcp` through their runtime's existing
 * MCP client instead of importing a language MCP facade.
 */
import { CuaDriver, SdkClientKind } from "./native/cua_driver_sdk.js"

// The same native library backs Python and TypeScript. Keep the public
// CuaDriver.connect signature stable while attaching the importing runtime as
// a closed, content-free category to direct daemon requests.
CuaDriver.connect = (socketPath: string | undefined) =>
  CuaDriver.connectWithClientKind(socketPath, SdkClientKind.Typescript)

export * from "./native/index.js"
export { default } from "./native/index.js"
