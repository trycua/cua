/**
 * Calculator addition tests — TypeScript async, Vitest.
 *
 * This file runs in its own Vitest fork alongside calculator-multiplication.test.ts.
 * Each fork launches a separate Calculator instance and cua-driver subprocess,
 * so the two suites execute concurrently without interfering.
 *
 * Run a single file:
 *   npx vitest run calculator-addition.test.ts
 *
 * Run in parallel with multiplication tests:
 *   npx vitest run
 */

import { beforeAll, afterAll, beforeEach, describe, it } from 'vitest'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'

import { DriverClient, App, expect } from '@cua/driver-sdk'
import type { Window } from '@cua/driver-sdk'

const execFileAsync = promisify(execFile)

// ---------------------------------------------------------------------------
// Suite setup — one Calculator instance for the whole file
// ---------------------------------------------------------------------------

let client: DriverClient
let app: App
let window: Window

beforeAll(async () => {
  // Kill any stale Calculator
  await execFileAsync('pkill', ['-x', 'Calculator']).catch(() => {})
  await new Promise((r) => setTimeout(r, 400))

  client = await DriverClient.create()
  app = await App.launch('com.apple.calculator', client, { settle: 1500 })
  window = await app.mainWindow()
}, 30_000)

afterAll(async () => {
  await app?.quit()
  await client?.close()
  await execFileAsync('pkill', ['-x', 'Calculator']).catch(() => {})
})

// Clear between tests
beforeEach(async () => {
  try {
    await window.locator('AXButton', { label: 'All Clear' }).click(2000)
  } catch {
    try {
      await window.locator('AXButton', { label: 'Clear' }).click(2000)
    } catch { /* already at 0 */ }
  }
})

// ---------------------------------------------------------------------------
// Tests — sequential within this file, parallel with other files
// ---------------------------------------------------------------------------

describe('Calculator addition', () => {
  it('2 + 2 = 4', async () => {
    await window.locator('AXButton', { label: '2' }).click()
    await window.locator('AXButton', { label: 'Add' }).click()
    await window.locator('AXButton', { label: '2' }).click()
    await window.locator('AXButton', { label: 'Equals' }).click()
    await expect(window.locator('AXStaticText')).toContainText('4')
  })

  it('1 + 1 = 2', async () => {
    await window.locator('AXButton', { label: '1' }).click()
    await window.locator('AXButton', { label: 'Add' }).click()
    await window.locator('AXButton', { label: '1' }).click()
    await window.locator('AXButton', { label: 'Equals' }).click()
    await expect(window.locator('AXStaticText')).toContainText('2')
  })

  it('3 + 4 = 7', async () => {
    await window.locator('AXButton', { label: '3' }).click()
    await window.locator('AXButton', { label: 'Add' }).click()
    await window.locator('AXButton', { label: '4' }).click()
    await window.locator('AXButton', { label: 'Equals' }).click()
    await expect(window.locator('AXStaticText')).toContainText('7')
  })

  it('9 + 9 = 18', async () => {
    await window.locator('AXButton', { label: '9' }).click()
    await window.locator('AXButton', { label: 'Add' }).click()
    await window.locator('AXButton', { label: '9' }).click()
    await window.locator('AXButton', { label: 'Equals' }).click()
    await expect(window.locator('AXStaticText')).toContainText('18')
  })
})
