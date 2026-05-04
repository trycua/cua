/**
 * Calculator multiplication tests — TypeScript async, Vitest.
 *
 * Runs CONCURRENTLY with calculator-addition.test.ts in a separate fork.
 * Each fork has its own Calculator instance and cua-driver process.
 *
 * Run in parallel:
 *   npx vitest run   (from sdk/examples/typescript/)
 */

import { beforeAll, afterAll, beforeEach, describe, it } from 'vitest'
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'

import { DriverClient, App, expect } from '@cua/driver-sdk'
import type { Window } from '@cua/driver-sdk'

const execFileAsync = promisify(execFile)

// ---------------------------------------------------------------------------
// Suite setup
// ---------------------------------------------------------------------------

let client: DriverClient
let app: App
let window: Window

beforeAll(async () => {
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
// Tests
// ---------------------------------------------------------------------------

describe('Calculator multiplication', () => {
  it('2 × 3 = 6', async () => {
    await window.locator('AXButton', { label: '2' }).click()
    await window.locator('AXButton', { label: 'Multiply' }).click()
    await window.locator('AXButton', { label: '3' }).click()
    await window.locator('AXButton', { label: 'Equals' }).click()
    await expect(window.locator('AXStaticText')).toContainText('6')
  })

  it('4 × 4 = 16', async () => {
    await window.locator('AXButton', { label: '4' }).click()
    await window.locator('AXButton', { label: 'Multiply' }).click()
    await window.locator('AXButton', { label: '4' }).click()
    await window.locator('AXButton', { label: 'Equals' }).click()
    await expect(window.locator('AXStaticText')).toContainText('16')
  })

  it('5 × 3 = 15', async () => {
    await window.locator('AXButton', { label: '5' }).click()
    await window.locator('AXButton', { label: 'Multiply' }).click()
    await window.locator('AXButton', { label: '3' }).click()
    await window.locator('AXButton', { label: 'Equals' }).click()
    await expect(window.locator('AXStaticText')).toContainText('15')
  })

  it('7 × 8 = 56', async () => {
    await window.locator('AXButton', { label: '7' }).click()
    await window.locator('AXButton', { label: 'Multiply' }).click()
    await window.locator('AXButton', { label: '8' }).click()
    await window.locator('AXButton', { label: 'Equals' }).click()
    await expect(window.locator('AXStaticText')).toContainText('56')
  })
})
