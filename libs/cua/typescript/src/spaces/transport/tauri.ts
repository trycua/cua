/**
 * `TauriTransport` — the conformer that makes this SDK usable from a webview.
 *
 * A Tauri frontend cannot load the package's N-API binding, so the root
 * export cannot run there. What it *can* do is `invoke()` a Rust command
 * that hands the JSON-RPC message to the Rust Spaces MCP server (in process
 * via `cua_spaces::mcp::McpServer::handle`, or the daemon), and this class
 * is the lines that carry a message across that call.
 *
 * Everything above it — `McpSession`, the handshake, the tool vocabulary, the
 * `isError` unwrapping — is the same code a Node consumer runs. That is the
 * claim the seam exists to make true.
 *
 * # Why `invoke` is injected rather than imported
 *
 * This package does not depend on `@tauri-apps/api`, and should not: a Node
 * or browser consumer installing the Spaces SDK should not acquire a desktop
 * framework. The host passes its own `invoke`, which also makes the transport
 * trivially testable with a function that returns canned replies — the
 * conformance test does exactly that.
 */

import {
  type Json,
  type RpcOutgoing,
  type RpcResponse,
  type SendOptions,
  type Transport,
  TransportError,
} from "./types.js"

/** The shape of Tauri's `invoke`, narrowed to what this needs. */
export type InvokeFn = <T>(command: string, args?: Record<string, unknown>) => Promise<T>

export interface TauriOptions {
  /** Usually `(await import("@tauri-apps/api/core")).invoke`. */
  invoke: InvokeFn
  /** The Rust command name (default `spaces_mcp_request`). */
  requestCommand?: string
  /** The Rust command that ends the session (default `spaces_mcp_shutdown`). */
  shutdownCommand?: string
}

export const DEFAULT_REQUEST_COMMAND = "spaces_mcp_request"
export const DEFAULT_SHUTDOWN_COMMAND = "spaces_mcp_shutdown"

export class TauriTransport implements Transport {
  readonly kind = "tauri"
  private readonly invoke: InvokeFn
  private readonly requestCommand: string
  private readonly shutdownCommand: string

  constructor(options: TauriOptions) {
    this.invoke = options.invoke
    this.requestCommand = options.requestCommand ?? DEFAULT_REQUEST_COMMAND
    this.shutdownCommand = options.shutdownCommand ?? DEFAULT_SHUTDOWN_COMMAND
  }

  async send(message: RpcOutgoing, options: SendOptions = {}): Promise<RpcResponse | null> {
    if (options.signal?.aborted) {
      throw new TransportError(`${message.method} was aborted`)
    }

    // The Rust side owns the wait, including the timeout, because it owns the
    // pending-reply map: a deadline enforced only here would leave the bridge
    // holding a waiter for a reply nobody wants any more.
    const call = this.invoke<Json>(this.requestCommand, {
      message: message as unknown as Json,
      timeoutSecs: options.timeoutSeconds ?? null,
    })

    const value = options.signal ? await race(call, options.signal, message.method) : await call

    // A notification gets no reply, and the bridge answers `null` for it
    // rather than inventing one.
    if (value === null || value === undefined) return null
    if (typeof value !== "object" || Array.isArray(value)) {
      throw new TransportError(`${message.method}: the bridge returned a non-object reply`)
    }
    return value as unknown as RpcResponse
  }

  /**
   * Ask the bridge to close the server's stdin.
   *
   * A failure here is swallowed: the transport is being torn down, the app may
   * already be quitting, and turning "could not reach the bridge while closing"
   * into an exception makes shutdown paths fragile for no gain.
   */
  async close(): Promise<void> {
    try {
      await this.invoke<void>(this.shutdownCommand)
    } catch {
      /* the bridge is gone, which is the state we wanted */
    }
  }
}

/**
 * Abort support for a call we cannot actually cancel.
 *
 * The server has no `notifications/cancelled` handling and is strictly
 * sequential, so aborting abandons the reply — the work still runs to
 * completion. This is honest about that rather than implying a cancel: the
 * promise rejects, the bridge's waiter is left to its own timeout.
 */
async function race<T>(promise: Promise<T>, signal: AbortSignal, method: string): Promise<T> {
  return await Promise.race([
    promise,
    new Promise<never>((_resolve, reject) => {
      const onAbort = () => reject(new TransportError(`${method} was abandoned`))
      if (signal.aborted) return onAbort()
      signal.addEventListener("abort", onAbort, { once: true })
    }),
  ])
}
