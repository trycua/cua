/**
 * The transport seam.
 *
 * # Why a seam at all
 *
 * The Spaces control plane is the Rust MCP server (`cua daemon mcp` over
 * stdio, the daemon's `/mcp` over HTTP). Three of the four places this SDK
 * has to run cannot spawn a subprocess:
 *
 * | host | can spawn a subprocess? | can load the N-API binding? |
 * |---|---|---|
 * | Node | yes | yes |
 * | Tauri webview (`apps/cua-spaces`) | no | no |
 * | browser | no | no |
 * | React Native | no | yes, via UBRN |
 *
 * So a TypeScript SDK that owns its own stdio transport can only ever run in
 * Node, and "the TypeScript SDK" would not be the thing our own app uses. The
 * transport is what has to be pluggable for the same API to be true
 * everywhere; everything above it — the handshake, the tool vocabulary, the
 * error shape — is identical whatever carries the bytes.
 *
 * # Where the seam falls, and why it is not at the byte level
 *
 * A `Transport` exchanges **whole JSON-RPC messages**, not bytes. That is the
 * load-bearing choice.
 *
 * Line framing is protocol. It is the bug the architecture document calls §1
 * — a framer that dropped every byte after the first newline in a chunk — and
 * it is invisible until it is catastrophic. If the seam were a byte stream,
 * every host would re-implement that framer, and the whole argument for one
 * Rust core would be conceded at exactly the layer that has already been got
 * wrong once.
 *
 * At the message level the framer lives wherever the server lives: in the
 * Rust `cua_spaces::mcp::stdio` framer, the daemon's HTTP endpoint, or the
 * Tauri app's Rust side. None of them is the renderer's problem.
 *
 * # Nothing in this file imports the native binding
 *
 * This module and everything under `src/transport/` is dependency-free,
 * platform-free TypeScript. It is exported from `@trycua/cua/spaces/transport`
 * rather than the package root precisely so a webview bundle can import it
 * without dragging in `@ubjs/node`, which would fail to resolve and take the
 * whole bundle with it.
 */

/** A JSON value, as it crosses the wire. */
export type Json = null | boolean | number | string | Json[] | { [key: string]: Json }

/** JSON-RPC 2.0 ids: this SDK only ever sends numbers, but a reply echoes
 * whatever it was given, and a conforming peer may use a string. */
export type RpcId = number | string

export interface RpcRequest {
  jsonrpc: "2.0"
  id: RpcId
  method: string
  params?: Json
}

/** A message with no `id`. It gets no reply, and a transport must resolve
 * `null` for it rather than inventing one. */
export interface RpcNotification {
  jsonrpc: "2.0"
  method: string
  params?: Json
}

export interface RpcError {
  code: number
  message: string
  data?: Json
}

export interface RpcResponse {
  jsonrpc: "2.0"
  id: RpcId
  result?: Json
  error?: RpcError
}

export type RpcOutgoing = RpcRequest | RpcNotification

export interface SendOptions {
  /**
   * Seconds to wait for a reply.
   *
   * The default is deliberately long. `teleport_app` and provisioning run with
   * `timeout=900` server-side, and the agent-CLI registration this server
   * ships with sets `requestTimeoutMs: 900000`. A transport that imposed a
   * short deadline would silently break the app's slowest and most valuable
   * operations, so a conformer must not shorten this on its own initiative.
   */
  timeoutSeconds?: number
  /** Aborts the wait. A transport should still expect the server to finish the
   * work: this server has no `notifications/cancelled` handling, so aborting
   * abandons the reply rather than stopping the call. */
  signal?: AbortSignal
}

/**
 * The one interface a host has to implement.
 *
 * Implementations must:
 * - resolve the reply whose `id` matches the request's, and not some other
 *   reply that happened to arrive first;
 * - resolve `null` for a notification;
 * - reject every in-flight call if the peer dies, rather than leaving callers
 *   hanging on a reply that is never coming;
 * - be safe to call again after `close()`, either by restarting or by
 *   rejecting clearly.
 */
export interface Transport {
  /** A short name for diagnostics — `"stdio"`, `"tauri"`. */
  readonly kind: string
  /** Send one message; resolve its reply, or `null` for a notification. */
  send(message: RpcOutgoing, options?: SendOptions): Promise<RpcResponse | null>
  /** Release the peer. Idempotent. */
  close(): Promise<void>
}

/** Thrown for anything that goes wrong beneath the tool call: a dead peer, a
 * timeout, a JSON-RPC `error` object. A failing *tool* is not this — see
 * `ToolError`, because this server reports tool failure as a successful
 * response carrying `isError: true`. */
export class TransportError extends Error {
  readonly tag = "TransportError"
  readonly code: number | undefined
  constructor(message: string, code?: number) {
    super(message)
    this.name = "TransportError"
    this.code = code
  }
}
