/**
 * `McpSession` — the MCP vocabulary, over any {@link Transport}.
 *
 * This is the piece that makes the seam worth having: it is the same code in
 * Node, in a Tauri webview and in a browser, and the only thing that differs
 * between them is which transport it was constructed with.
 *
 * It is deliberately small. It owns the handshake, id allocation, and the
 * unwrapping of the `content` / `isError` envelope — and nothing else. The
 * server on the other side is the Rust Spaces MCP server (`cua daemon mcp`,
 * the daemon's `/mcp`, or the Tauri app's bridge); its tools are the Spaces
 * contract (`libs/cua/spaces-contract/manifest.json`). Node code that can load
 * the native binding uses `@trycua/cua/spaces` (typed) instead.
 */

import {
  type Json,
  type RpcId,
  type RpcResponse,
  type SendOptions,
  type Transport,
  TransportError,
} from "./types.js"

/** The MCP revision offered. The Rust server accepts 2025-06-18, 2025-03-26
 * and 2024-11-05, and answers with the one it picked. */
export const PROTOCOL_VERSION = "2025-06-18"

export interface ClientInfo {
  name: string
  version: string
}

export interface ServerInfo {
  name: string
  version: string
}

/** One part of a tool result. `call_tool` passes through `image`,
 * `audio` and `resource` parts from an in-space tool unchanged, so this is not
 * only ever text. */
export interface ContentPart {
  type: string
  text?: string
  [key: string]: Json | undefined
}

export interface ToolResult {
  content: ContentPart[]
  isError: boolean
  /** `structuredContent`, when the tool returned one (tool errors carry
   * `{error: {kind, message}}` with a stable `kind`). */
  structured?: Json
}

export interface ToolDescriptor {
  name: string
  description?: string
  inputSchema?: Json
}

/**
 * A tool that ran and failed.
 *
 * This is a separate type from {@link TransportError} on purpose. The server
 * reports a failing tool as a *successful* JSON-RPC response carrying
 * `isError: true` — the only JSON-RPC `error` objects it ever sends are
 * `-32601` for an unknown method or tool. Collapsing the two would make
 * "the sandbox refused this command" indistinguishable from "the control
 * plane is not running", and those need different handling by every caller.
 */
export class ToolError extends Error {
  readonly tag = "ToolError"
  readonly tool: string
  readonly content: ContentPart[]
  /** The server's stable error kind (`capability_missing`,
   * `teleport_refused`, `not_found`, ...), when it sent one. */
  readonly kind: string | undefined
  constructor(tool: string, content: ContentPart[], structured?: Json) {
    super(`${tool}: ${textOf(content) || "failed"}`)
    this.name = "ToolError"
    this.tool = tool
    this.content = content
    const error = (structured as { error?: { kind?: unknown } } | null | undefined)?.error
    this.kind = typeof error?.kind === "string" ? error.kind : undefined
  }
}

/** Concatenate the text parts, which is what almost every caller wants. */
export function textOf(content: ContentPart[]): string {
  return content
    .filter((part) => part.type === "text" && typeof part.text === "string")
    .map((part) => part.text as string)
    .join("\n")
}

export interface SessionOptions {
  clientInfo?: ClientInfo
}

export class McpSession {
  readonly transport: Transport
  private nextId = 1
  private handshake: Promise<ServerInfo | undefined> | undefined
  private readonly clientInfo: ClientInfo

  constructor(transport: Transport, options: SessionOptions = {}) {
    this.transport = transport
    this.clientInfo = options.clientInfo ?? { name: "@trycua/cua", version: "0.2.0" }
  }

  private allocateId(): RpcId {
    return this.nextId++
  }

  private unwrap(response: RpcResponse | null, method: string): Json {
    if (response === null) {
      throw new TransportError(`${method} expected a reply and got none`)
    }
    if (response.error) {
      throw new TransportError(`${method}: ${response.error.message}`, response.error.code)
    }
    return response.result ?? null
  }

  /**
   * Perform the MCP handshake, at most once per session.
   *
   * The server does not require this — it builds its dispatch table before the
   * read loop starts, so `tools/call` works cold. We do it anyway, and we
   * memoize it: it is what makes this session usable against a *conforming*
   * MCP server rather than only against this one, and the cost is a single
   * round trip.
   *
   * The result is cached as a promise, not a value, so concurrent first calls
   * share one handshake instead of racing two.
   */
  async initialize(options?: SendOptions): Promise<ServerInfo | undefined> {
    if (!this.handshake) {
      this.handshake = this.performHandshake(options).catch((error) => {
        // A failed handshake must not poison the session forever; a later call
        // should be free to try again against a restarted server.
        this.handshake = undefined
        throw error
      })
    }
    return this.handshake
  }

  private async performHandshake(options?: SendOptions): Promise<ServerInfo | undefined> {
    const result = this.unwrap(
      await this.transport.send(
        {
          jsonrpc: "2.0",
          id: this.allocateId(),
          method: "initialize",
          params: {
            protocolVersion: PROTOCOL_VERSION,
            capabilities: {},
            clientInfo: { ...this.clientInfo },
          },
        },
        options,
      ),
      "initialize",
    )
    await this.transport.send({ jsonrpc: "2.0", method: "notifications/initialized" }, options)
    const info = (result as { serverInfo?: ServerInfo } | null)?.serverInfo
    return info
  }

  /** Sends a request after the handshake; re-initializes once when the
   * server says the session expired (HTTP 404 from a restarted daemon). */
  private async request(method: string, params: Json | undefined, options?: SendOptions): Promise<Json> {
    for (let attempt = 0; ; attempt++) {
      await this.initialize(options)
      try {
        const message =
          params === undefined
            ? { jsonrpc: "2.0" as const, id: this.allocateId(), method }
            : { jsonrpc: "2.0" as const, id: this.allocateId(), method, params }
        return this.unwrap(await this.transport.send(message, options), method)
      } catch (error) {
        if (attempt === 0 && error instanceof TransportError && error.code === 404) {
          this.handshake = undefined
          continue
        }
        throw error
      }
    }
  }

  /** Every tool the server serves, with its input schema. */
  async listTools(options?: SendOptions): Promise<ToolDescriptor[]> {
    const result = await this.request("tools/list", undefined, options)
    const tools = (result as { tools?: ToolDescriptor[] } | null)?.tools
    return Array.isArray(tools) ? tools : []
  }

  /**
   * Call a tool and return its envelope, including a failure.
   *
   * Use this when a caller wants to inspect `isError` itself — for instance a
   * probe that treats "not available" as an answer rather than an exception.
   * Most callers want {@link call}.
   */
  async callRaw(
    name: string,
    args: Record<string, Json> = {},
    options?: SendOptions,
  ): Promise<ToolResult> {
    const result = (await this.request("tools/call", { name, arguments: args }, options)) as {
      content?: ContentPart[]
      isError?: boolean
      structuredContent?: Json
    } | null

    const content = Array.isArray(result?.content) ? result!.content : []
    // `isError` is read as a field and never inferred from the shape of the
    // content: a missing flag means the server did not say, and reading
    // silence as success is how a failure becomes a wrong answer instead of an
    // error. This server always sends it explicitly, including `false`.
    const out: ToolResult = { content, isError: result?.isError === true }
    if (result?.structuredContent !== undefined) out.structured = result.structuredContent
    return out
  }

  /** Call a tool, throwing {@link ToolError} if it failed. */
  async call(
    name: string,
    args: Record<string, Json> = {},
    options?: SendOptions,
  ): Promise<ContentPart[]> {
    const result = await this.callRaw(name, args, options)
    if (result.isError) throw new ToolError(name, result.content, result.structured)
    return result.content
  }

  /** Call a tool and return its text parts joined — the common case, since
   * most of these tools answer with one text part. */
  async callText(
    name: string,
    args: Record<string, Json> = {},
    options?: SendOptions,
  ): Promise<string> {
    return textOf(await this.call(name, args, options))
  }

  /**
   * Call a tool whose text part is JSON, and parse it.
   *
   * Worth its own method because the parse failure needs to say which tool
   * produced the unparsable text — without that, a server-side format change
   * surfaces as a bare `SyntaxError` with no attribution.
   */
  async callJson<T = Json>(
    name: string,
    args: Record<string, Json> = {},
    options?: SendOptions,
  ): Promise<T> {
    const text = await this.callText(name, args, options)
    try {
      return JSON.parse(text) as T
    } catch {
      throw new TransportError(`${name} did not return JSON: ${text.slice(0, 200)}`)
    }
  }

  async close(): Promise<void> {
    this.handshake = undefined
    await this.transport.close()
  }
}
