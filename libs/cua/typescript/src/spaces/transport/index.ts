/**
 * `@trycua/cua/spaces/transport`: the Spaces control plane for hosts that
 * cannot load the native binding (a Tauri webview, a browser).
 *
 * Dependency-free TypeScript: `McpSession` speaks MCP to the Rust Spaces
 * server over any `Transport`:
 *
 * - `McpHttpTransport`: the `cua daemon` loopback `/mcp` (streamable HTTP,
 *   bearer = the daemon's loopback token);
 * - `TauriTransport`: `invoke()` into the Tauri app's Rust side.
 *
 * Node code should use `@trycua/cua/spaces` (the typed SDK) instead. See
 * `types.ts` for why the seam exchanges whole JSON-RPC messages rather than
 * bytes.
 */

export {
  type Json,
  type RpcError,
  type RpcId,
  type RpcNotification,
  type RpcOutgoing,
  type RpcRequest,
  type RpcResponse,
  type SendOptions,
  type Transport,
  TransportError,
} from "./types.js"

export {
  type ClientInfo,
  type ContentPart,
  McpSession,
  PROTOCOL_VERSION,
  type ServerInfo,
  type SessionOptions,
  ToolError,
  type ToolDescriptor,
  type ToolResult,
  textOf,
} from "./session.js"

export {
  DEFAULT_REQUEST_COMMAND,
  DEFAULT_SHUTDOWN_COMMAND,
  type InvokeFn,
  TauriTransport,
  type TauriOptions,
} from "./tauri.js"

export { type FetchFn, McpHttpTransport, type McpHttpOptions, SESSION_HEADER } from "./http.js"
