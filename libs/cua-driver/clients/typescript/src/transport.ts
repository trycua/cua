import { spawn, type ChildProcess } from "node:child_process"
import { createInterface, type Interface } from "node:readline"

import { MCP_PROTOCOL_VERSION } from "./generated.js"

export interface Transport {
  request(method: string, params?: Record<string, unknown>): Promise<Record<string, unknown>>
  close(): Promise<void>
}

export class McpResponseError extends Error {
  constructor(
    readonly code: number | null,
    message: string,
  ) {
    super(message)
    this.name = "McpResponseError"
  }
}

type Pending = {
  resolve(value: Record<string, unknown>): void
  reject(error: Error): void
}

function record(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}

export class StdioMcpTransport implements Transport {
  private child: ChildProcess | null = null
  private lines: Interface | null = null
  private nextId = 1
  private pending = new Map<number, Pending>()
  private startPromise: Promise<void> | null = null

  constructor(
    private readonly command: readonly string[] = ["cua-driver", "mcp"],
    private readonly options: { cwd?: string; env?: NodeJS.ProcessEnv } = {},
  ) {
    if (command.length === 0) throw new TypeError("command must not be empty")
  }

  async request(
    method: string,
    params?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    await this.ensureStarted()
    return this.exchange(method, params)
  }

  async close(): Promise<void> {
    const child = this.child
    this.child = null
    this.startPromise = null
    this.lines?.close()
    this.lines = null
    if (!child) return
    child.stdin?.end()
    if (child.exitCode === null) child.kill("SIGTERM")
    for (const pending of this.pending.values()) {
      pending.reject(new Error("cua-driver MCP transport closed"))
    }
    this.pending.clear()
  }

  private async ensureStarted(): Promise<void> {
    if (this.startPromise) return this.startPromise
    if (this.child) return
    this.startPromise = this.start()
    return this.startPromise
  }

  private async start(): Promise<void> {
    const [executable, ...args] = this.command
    if (!executable) throw new TypeError("command must not be empty")
    const child = spawn(executable, args, {
      cwd: this.options.cwd,
      env: { ...process.env, ...this.options.env },
      stdio: ["pipe", "pipe", "inherit"],
    })
    this.child = child
    if (!child.stdout || !child.stdin) throw new Error("failed to open cua-driver MCP pipes")
    this.lines = createInterface({ input: child.stdout })
    this.lines.on("line", line => this.handleLine(line))
    child.on("error", error => this.failAll(error))
    child.on("exit", code => this.failAll(new Error(`cua-driver MCP exited (code=${code})`)))
    try {
      await this.exchange("initialize", {
        protocolVersion: MCP_PROTOCOL_VERSION,
        capabilities: {},
        clientInfo: { name: "cua-driver-typescript-client", version: "0.0.0" },
      })
      this.notify("notifications/initialized")
    } catch (error) {
      await this.close()
      throw error
    }
  }

  private exchange(
    method: string,
    params?: Record<string, unknown>,
  ): Promise<Record<string, unknown>> {
    const id = this.nextId++
    const payload: Record<string, unknown> = { jsonrpc: "2.0", id, method }
    if (params !== undefined) payload.params = params
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject })
      try {
        this.write(payload)
      } catch (error) {
        this.pending.delete(id)
        reject(error instanceof Error ? error : new Error(String(error)))
      }
    })
  }

  private notify(method: string, params?: Record<string, unknown>): void {
    const payload: Record<string, unknown> = { jsonrpc: "2.0", method }
    if (params !== undefined) payload.params = params
    this.write(payload)
  }

  private write(payload: Record<string, unknown>): void {
    if (!this.child?.stdin) throw new Error("cua-driver MCP transport is not running")
    this.child.stdin.write(`${JSON.stringify(payload)}\n`)
  }

  private handleLine(line: string): void {
    let value: unknown
    try {
      value = JSON.parse(line)
    } catch {
      this.failAll(new Error("cua-driver MCP emitted invalid JSON"))
      return
    }
    const response = record(value)
    const id = typeof response?.id === "number" ? response.id : null
    if (id === null) return
    const pending = this.pending.get(id)
    if (!pending) return
    this.pending.delete(id)
    const error = record(response?.error)
    if (error) {
      pending.reject(
        new McpResponseError(
          typeof error.code === "number" ? error.code : null,
          typeof error.message === "string" ? error.message : JSON.stringify(error),
        ),
      )
      return
    }
    const result = record(response?.result)
    if (!result) {
      pending.reject(new McpResponseError(null, "MCP response has no object result"))
      return
    }
    pending.resolve(result)
  }

  private failAll(error: Error): void {
    for (const pending of this.pending.values()) pending.reject(error)
    this.pending.clear()
  }
}
