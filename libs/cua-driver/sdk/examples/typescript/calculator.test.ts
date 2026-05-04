/**
 * Calculator tests (addition + multiplication) — TypeScript async, Vitest.
 *
 * Runs CONCURRENTLY with safari-form.test.ts in a separate Vitest fork.
 * Calculator and Safari are different apps so they don't interfere.
 *
 * Run just this file:
 *   npx vitest run calculator.test.ts
 *
 * Run in parallel with Safari tests:
 *   npx vitest run
 */

import { beforeAll, afterAll, beforeEach, describe, it } from 'vitest'

import { DriverClient, App, expect, killApp } from '@cua/driver-sdk'
import type { Window } from '@cua/driver-sdk'

// ---------------------------------------------------------------------------
// Suite setup — one Calculator instance for the whole file
// ---------------------------------------------------------------------------

let client: DriverClient
let app: App
let window: Window

beforeAll(async () => {
  await killApp('com.apple.calculator')
  await new Promise((r) => setTimeout(r, 400))

  client = await DriverClient.create()
  app = await App.launch('com.apple.calculator', client, { settle: 1500 })
  window = await app.mainWindow()
}, 30_000)

afterAll(async () => {
  await app?.quit()
  await client?.close()
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
// Addition tests
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

// ---------------------------------------------------------------------------
// Multiplication tests
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
