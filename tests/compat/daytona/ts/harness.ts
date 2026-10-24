// Shared-case conformance harness for @daytonaio/sdk. A case registers once with
// `testCase(id, needs, fn)` under the same id as its Python twin (see ../cases.json)
// and returns a JSON-able observation that must equal the Python one and the golden.
import { Daytona, DaytonaConfig } from '@daytonaio/sdk'
import * as sdk from '@daytonaio/sdk'

export type Needs = 'mock' | 'space' | 'hermetic'
export interface Case {
  id: string
  needs: Needs
  fn: (c: Ctx) => Promise<unknown>
  timeout: number
  requires: string
}
export const CASES = new Map<string, Case>()

export function testCase(id: string, needs: Needs, fn: (c: Ctx) => Promise<unknown>, timeoutSec = 180, requires = ''): void {
  CASES.set(id, { id, needs, fn, timeout: timeoutSec * 1000, requires })
}

export class Ctx {
  sdk = sdk
  made: any[] = []
  private mainSb: any
  runId = `${process.pid}${Math.floor(Date.now() / 1000) % 100000}`
  constructor(public daytona: Daytona) {}

  async newSandbox(params: any = {}, options: any = { timeout: 300 }): Promise<any> {
    const sb = await this.daytona.create(params, options)
    this.made.push(sb)
    return sb
  }
  /** The suite's shared sandbox (created on first use). */
  async main(): Promise<any> {
    if (!this.mainSb) {
      this.mainSb = await this.newSandbox({ labels: { suite: 'conformance', run: this.runId }, autoStopInterval: 0 })
    }
    return this.mainSb
  }
  async drop(sb: any): Promise<void> {
    try {
      await this.daytona.delete(sb)
    } catch {
      /* gone already */
    }
    this.made = this.made.filter((s) => s !== sb)
  }
  async cleanup(): Promise<void> {
    for (const sb of [...this.made]) await this.drop(sb)
  }
  name(stem: string): string {
    return `${stem}-${this.runId}-${Date.now() % 100000}`
  }
  /** Calls `fn`; returns the error as an observation (null when it did not throw). */
  async raises(fn: () => Promise<unknown>): Promise<ErrObs | null> {
    try {
      await fn()
    } catch (e) {
      return describeError(e)
    }
    return null
  }
}

export interface ErrObs {
  type: string
  status_code?: number
  code?: string
  source?: string
  message: string
}

/** Python-shaped error observation (class name, status, code, source, message). */
// The TypeScript SDK raises deprecated subclasses where Python raises the base
// class (`DaytonaValidationError` extends `DaytonaBadRequestError`, same for
// authorization); observations use the canonical class.
const CANONICAL: Record<string, string> = {
  DaytonaValidationError: 'DaytonaBadRequestError',
  DaytonaAuthorizationError: 'DaytonaForbiddenError',
  DaytonaInvalidArgumentError: 'DaytonaBadRequestError',
}

export function describeError(e: any): ErrObs {
  const name = e?.constructor?.name ?? 'Error'
  const o: ErrObs = { type: CANONICAL[name] ?? name, message: String(e?.message ?? e) }
  if (e?.statusCode !== undefined && e.statusCode !== null) o.status_code = e.statusCode
  if (e?.errorCode) o.code = e.errorCode
  else if (e?.code) o.code = e.code
  if (e?.source) o.source = e.source
  return o
}

export function errObs(o: ErrObs | null): Omit<ErrObs, 'message'> | null {
  if (!o) return null
  const { message: _m, ...rest } = o
  return rest
}

export function assert(cond: unknown, msg: string): asserts cond {
  if (!cond) throw new Error(`assertion failed: ${msg}`)
}
export function eq(a: unknown, b: unknown, what: string): void {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${what}: ${JSON.stringify(a)} != ${JSON.stringify(b)}`)
}

export async function runAll(selected: string[], needs: Set<string>, emit: (m: string) => void): Promise<Record<string, any>> {
  const config: DaytonaConfig = {}
  const daytona = new Daytona(config)
  const ctx = new Ctx(daytona)
  const out: Record<string, any> = {}
  try {
    for (const id of selected) {
      const c = CASES.get(id)!
      if (!needs.has(c.needs)) continue
      if (c.requires && process.env[c.requires] !== '1') {
        out[id] = { ok: true, skipped: `needs ${c.requires}=1`, obs: null, seconds: 0 }
        emit(`SKIP ${id}: needs ${c.requires}=1`)
        continue
      }
      const t0 = Date.now()
      try {
        const obs = await Promise.race([
          c.fn(ctx),
          new Promise((_, rej) => setTimeout(() => rej(new Error(`timeout after ${c.timeout / 1000}s`)), c.timeout).unref()),
        ])
        out[id] = { ok: true, obs: obs === undefined ? null : obs, seconds: Math.round((Date.now() - t0) / 10) / 100 }
        emit(`PASS ${id} (${((Date.now() - t0) / 1000).toFixed(1)}s)`)
      } catch (e: any) {
        out[id] = { ok: false, error: `${e?.constructor?.name}: ${e?.message}`, seconds: Math.round((Date.now() - t0) / 10) / 100 }
        emit(`FAIL ${id}: ${e?.stack ?? e}`)
      }
    }
  } finally {
    await ctx.cleanup()
  }
  return out
}
