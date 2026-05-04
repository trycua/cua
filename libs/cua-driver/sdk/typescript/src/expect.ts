/**
 * expect() — assertion helper for cua-driver Locators.
 *
 * Retries assertions until the condition holds or the timeout elapses.
 *
 * Usage:
 * ```ts
 * import { expect } from './expect.js'
 *
 * await expect(window.locator('AXStaticText')).toContainText('4')
 * await expect(window.locator('AXButton', { label: 'OK' })).toBeVisible()
 * ```
 */

import type { Locator } from './locator.js'

const POLL_INTERVAL = 300  // ms

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms))
}

export class LocatorAssertions {
  constructor(
    private readonly locator: Locator,
    private readonly timeoutMs: number,
  ) {}

  async toBeVisible(timeoutMs?: number): Promise<void> {
    const t = timeoutMs ?? this.timeoutMs
    const deadline = Date.now() + t
    while (true) {
      if (await this.locator.isVisible(0)) return
      if (Date.now() >= deadline) throw new Error(`Element not visible after ${t}ms`)
      await sleep(POLL_INTERVAL)
    }
  }

  async notToBeVisible(timeoutMs?: number): Promise<void> {
    const t = timeoutMs ?? this.timeoutMs
    const deadline = Date.now() + t
    while (true) {
      if (!await this.locator.isVisible(0)) return
      if (Date.now() >= deadline) throw new Error(`Element still visible after ${t}ms`)
      await sleep(POLL_INTERVAL)
    }
  }

  async toContainText(text: string, timeoutMs?: number): Promise<void> {
    const t = timeoutMs ?? this.timeoutMs
    const deadline = Date.now() + t
    let last = '<not found>'
    while (true) {
      // Check ALL matching elements — needed when multiple elements share a role
      // (e.g. Calculator result + expression-history are both AXStaticText).
      const contents = await this.locator.allTextContents()
      for (const content of contents) {
        if (content.includes(text)) return
      }
      if (contents.length) last = JSON.stringify(contents)
      if (Date.now() >= deadline) {
        throw new Error(`Expected text "${text}" not found in ${last} after ${t}ms`)
      }
      await sleep(POLL_INTERVAL)
    }
  }

  async toHaveText(text: string, timeoutMs?: number): Promise<void> {
    const t = timeoutMs ?? this.timeoutMs
    const deadline = Date.now() + t
    let last = '<not found>'
    while (true) {
      const contents = await this.locator.allTextContents()
      for (const content of contents) {
        if (content === text) return
      }
      if (contents.length) last = JSON.stringify(contents)
      if (Date.now() >= deadline) {
        throw new Error(`Expected text "${text}" but got ${last} after ${t}ms`)
      }
      await sleep(POLL_INTERVAL)
    }
  }

  async toHaveValue(value: string, timeoutMs?: number): Promise<void> {
    const t = timeoutMs ?? this.timeoutMs
    const deadline = Date.now() + t
    let last = '<not found>'
    while (true) {
      try {
        const val = await this.locator.getValue(0)
        if (val === value) return
        last = val
      } catch { /* element not found yet */ }
      if (Date.now() >= deadline) {
        throw new Error(`Expected value "${value}" but got "${last}" after ${t}ms`)
      }
      await sleep(POLL_INTERVAL)
    }
  }
}

/**
 * Return a {@link LocatorAssertions} for *locator*.
 *
 * @example
 * ```ts
 * await expect(window.locator('AXStaticText')).toContainText('4')
 * ```
 */
export function expect(locator: Locator, timeoutMs = 5_000): LocatorAssertions {
  return new LocatorAssertions(locator, timeoutMs)
}
