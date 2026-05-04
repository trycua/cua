/**
 * DriverClient — async MCP stdio client for cua-driver.
 *
 * Speaks JSON-RPC 2.0 line-framed over stdin/stdout of a spawned
 * ``cua-driver mcp`` subprocess.  All calls are Promise-based;
 * timeouts are enforced per-call.
 *
 * Usage:
 * ```ts
 * const client = new DriverClient()
 * await client.start()
 * try {
 *   const result = await client.callTool('list_apps')
 * } finally {
 *   await client.close()
 * }
 *
 * // Or with Symbol.asyncDispose (TypeScript 5.2 / Node 18):
 * await using client = await DriverClient.create()
 * const result = await client.callTool('list_apps')
 * ```
 */

import { spawn, ChildProcess } from 'node:child_process'
import { createInterface } from 'node:readline'
import * as path from 'node:path'
import * as fs from 'node:fs'

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

export interface MCPErrorBody {
  code: number
  message: string
}

export class MCPCallError extends Error {
  constructor(public readonly error: MCPErrorBody) {
    super(`MCP error ${error.code}: ${error.message}`)
    this.name = 'MCPCallError'
  }
}

export class DriverTimeoutError extends Error {
  constructor(method: string, timeoutMs: number) {
    super(`Timeout after ${timeoutMs}ms waiting for ${method}`)
    this.name = 'DriverTimeoutError'
  }
}

// ---------------------------------------------------------------------------
// Binary path
// ---------------------------------------------------------------------------

export function defaultBinaryPath(): string {
  const env = process.env['CUA_DRIVER_BINARY']
  if (env) return env
  // sdk/typescript/src/client.ts → sdk/typescript/ → sdk/ → libs/cua-driver/ → .build/debug/cua-driver
  return path.resolve(__dirname, '..', '..', '..', '.build', 'debug', 'cua-driver')
}

// ---------------------------------------------------------------------------
// DriverClient
// ---------------------------------------------------------------------------

export class DriverClient {
  private proc?: ChildProcess
  private nextId = 0
  private pending = new Map<
    number,
    { resolve: (v: unknown) => void; reject: (e: Error) => void }
  >()
  private readonly binaryPath: string
  private readonly subcommand: string

  constructor(binaryPath?: string, subcommand = 'mcp') {
    this.binaryPath = binaryPath ?? defaultBinaryPath()
    this.subcommand = subcommand
  }

  /** Create and start a client in one call. */
  static async create(binaryPath?: string): Promise<DriverClient> {
    const c = new DriverClient(binaryPath)
    await c.start()
    return c
  }

  async start(): Promise<void> {
    this.proc = spawn(this.binaryPath, [this.subcommand], {
      stdio: ['pipe', 'pipe', 'pipe'],
    })

    // Ensure the child is killed when the host Node process exits or is signalled.
    // Without this, cua-driver processes become orphans when the test runner
    // force-terminates a worker (e.g. on hook/test timeout).
    const killOnExit = () => { try { this.proc?.kill('SIGKILL') } catch {} }
    process.once('exit', killOnExit)
    process.once('SIGTERM', killOnExit)
    process.once('SIGINT', killOnExit)
    // Clean up these handlers when the child exits normally.
    this.proc.once('exit', () => {
      process.removeListener('exit', killOnExit)
      process.removeListener('SIGTERM', killOnExit)
      process.removeListener('SIGINT', killOnExit)
    })

    const rl = createInterface({ input: this.proc.stdout! })
    rl.on('line', (raw: string) => {
      const line = raw.trim()
      if (!line) return
      let msg: Record<string, unknown>
      try {
        msg = JSON.parse(line) as Record<string, unknown>
      } catch {
        return
      }
      const id = msg['id'] as number | undefined
      if (id !== undefined) {
        const handler = this.pending.get(id)
        if (handler) {
          this.pending.delete(id)
          if (msg['error']) {
            handler.reject(new MCPCallError(msg['error'] as MCPErrorBody))
          } else {
            handler.resolve(msg['result'] as unknown)
          }
        }
      }
    })

    await this.rpc('initialize', {
      protocolVersion: '2025-06-18',
      capabilities: {},
      clientInfo: { name: 'cua-driver-sdk-ts', version: '0.1.0' },
    })
    this.write({ jsonrpc: '2.0', method: 'notifications/initialized' })
  }

  async close(): Promise<void> {
    for (const [, h] of this.pending) {
      h.reject(new Error('DriverClient closed'))
    }
    this.pending.clear()
    this.proc?.stdin?.destroy()
    const proc = this.proc
    if (!proc) return
    proc.kill()  // SIGTERM
    await new Promise<void>((resolve) => {
      let done = false
      const finish = () => { if (!done) { done = true; resolve() } }
      proc.once('exit', finish)
      // After 1s, escalate to SIGKILL
      setTimeout(() => { try { proc.kill('SIGKILL') } catch {} }, 1000)
      // After 2s, give up — should already be dead from SIGKILL
      setTimeout(finish, 2000)
    })
  }

  /** Implement Symbol.asyncDispose for ``await using`` syntax. */
  async [Symbol.asyncDispose](): Promise<void> {
    await this.close()
  }

  async listTools(): Promise<unknown[]> {
    const result = await this.rpc('tools/list') as { tools: unknown[] }
    return result.tools
  }

  async callTool(
    name: string,
    args?: Record<string, unknown>,
    timeoutMs = 30_000,
  ): Promise<Record<string, unknown>> {
    return this.rpc(
      'tools/call',
      { name, arguments: args ?? {} },
      timeoutMs,
    ) as Promise<Record<string, unknown>>
  }

  // ------------------------------------------------------------------
  // Internal
  // ------------------------------------------------------------------

  private async rpc(
    method: string,
    params?: object,
    timeoutMs = 20_000,
  ): Promise<unknown> {
    const id = ++this.nextId
    const promise = new Promise<unknown>((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id)
        reject(new DriverTimeoutError(method, timeoutMs))
      }, timeoutMs)
      this.pending.set(id, {
        resolve: (v) => { clearTimeout(timer); resolve(v) },
        reject: (e) => { clearTimeout(timer); reject(e) },
      })
    })
    this.write({ jsonrpc: '2.0', id, method, params })
    return promise
  }

  private write(payload: object): void {
    this.proc!.stdin!.write(JSON.stringify(payload) + '\n')
  }
}
