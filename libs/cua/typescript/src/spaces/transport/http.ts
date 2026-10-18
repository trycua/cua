/**
 * `McpHttpTransport`: the Spaces MCP server over streamable HTTP, as served
 * by `cua daemon` at `<loopback>/mcp`.
 *
 * This is what a webview or browser uses when it can reach the daemon's
 * loopback listener: `POST /mcp` with one JSON-RPC message, answered with
 * `application/json`. The daemon mints an `Mcp-Session-Id` on `initialize`;
 * every later request carries it, and a 404 means the session is gone (the
 * daemon restarted), which this transport reports as a `TransportError` with
 * code 404 so `McpSession` re-initializes on the next call.
 *
 * The bearer is the daemon's loopback token (`GetInfo.loopback_token`, or the
 * `token` in `~/.cua/daemon.json`). Nothing here imports Node builtins.
 */

import {
  type Json,
  type RpcOutgoing,
  type RpcResponse,
  type SendOptions,
  type Transport,
  TransportError,
} from "./types.js"

/** The `fetch` shape this needs (the global one, or an injected fake). */
export type FetchFn = (
  input: string,
  init: { method: string; headers: Record<string, string>; body?: string; signal?: AbortSignal },
) => Promise<{
  status: number
  ok: boolean
  headers: { get(name: string): string | null }
  text(): Promise<string>
}>

export interface McpHttpOptions {
  /** Daemon loopback base URL (`http://127.0.0.1:<port>`) or the full `/mcp` URL. */
  url: string
  /** The daemon's loopback bearer token. */
  token?: string
  /** Inject a fetch (tests, a Tauri HTTP plugin). Defaults to `globalThis.fetch`. */
  fetch?: FetchFn
}

export const SESSION_HEADER = "mcp-session-id"

export class McpHttpTransport implements Transport {
  readonly kind = "http"
  private readonly endpoint: string
  private readonly token: string | undefined
  private readonly doFetch: FetchFn
  private session: string | undefined

  constructor(options: McpHttpOptions) {
    const base = options.url.replace(/\/+$/, "")
    this.endpoint = base.endsWith("/mcp") ? base : `${base}/mcp`
    this.token = options.token
    const f = options.fetch ?? (globalThis.fetch as unknown as FetchFn | undefined)
    if (!f) throw new TransportError("no fetch available; pass options.fetch")
    this.doFetch = f
  }

  /** The session id the daemon minted, once initialized. */
  get sessionId(): string | undefined {
    return this.session
  }

  async send(message: RpcOutgoing, options: SendOptions = {}): Promise<RpcResponse | null> {
    const headers: Record<string, string> = {
      "content-type": "application/json",
      accept: "application/json, text/event-stream",
    }
    if (this.token) headers.authorization = `Bearer ${this.token}`
    if (this.session && message.method !== "initialize") headers[SESSION_HEADER] = this.session
    const controller = new AbortController()
    const timer =
      options.timeoutSeconds === undefined
        ? undefined
        : setTimeout(() => controller.abort(), options.timeoutSeconds * 1000)
    const onAbort = () => controller.abort()
    options.signal?.addEventListener("abort", onAbort, { once: true })
    let response
    try {
      response = await this.doFetch(this.endpoint, {
        method: "POST",
        headers,
        body: JSON.stringify(message),
        signal: controller.signal,
      })
    } catch (cause) {
      throw new TransportError(`${message.method}: the daemon at ${this.endpoint} did not answer (${String(cause)})`)
    } finally {
      if (timer) clearTimeout(timer)
      options.signal?.removeEventListener("abort", onAbort)
    }
    const text = await response.text()
    if (response.status === 404 && this.session) {
      // The daemon forgot the session (it restarted): make the next call
      // initialize again.
      this.session = undefined
      throw new TransportError(`${message.method}: MCP session expired`, 404)
    }
    if (response.status === 401) {
      throw new TransportError(`${message.method}: the daemon refused the bearer token`, 401)
    }
    if (!response.ok && response.status !== 202) {
      throw new TransportError(`${message.method}: HTTP ${response.status} ${text.slice(0, 200)}`, response.status)
    }
    const minted = response.headers.get(SESSION_HEADER)
    if (minted) this.session = minted
    if (!("id" in message) || text.trim() === "") return null
    let parsed: Json
    try {
      parsed = JSON.parse(text) as Json
    } catch {
      throw new TransportError(`${message.method}: the daemon returned non-JSON: ${text.slice(0, 200)}`)
    }
    if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
      throw new TransportError(`${message.method}: the daemon returned a non-object reply`)
    }
    return parsed as unknown as RpcResponse
  }

  /** Ends the MCP session (`DELETE /mcp`). Never throws. */
  async close(): Promise<void> {
    if (!this.session) return
    const headers: Record<string, string> = { [SESSION_HEADER]: this.session }
    if (this.token) headers.authorization = `Bearer ${this.token}`
    this.session = undefined
    try {
      await this.doFetch(this.endpoint, { method: "DELETE", headers })
    } catch {
      /* the daemon is gone, which is the state we wanted */
    }
  }
}
