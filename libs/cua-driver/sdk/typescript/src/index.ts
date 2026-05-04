/**
 * cua-driver TypeScript SDK
 *
 * Playwright-style async API for macOS UI automation via cua-driver.
 *
 * Quick start:
 * ```ts
 * import { DriverClient, App, expect } from './index.js'
 *
 * const client = await DriverClient.create()
 * try {
 *   await using app = await App.launch('com.apple.calculator', client)
 *   const window = await app.mainWindow()
 *   await window.locator('AXButton', { label: '2' }).click()
 *   await window.locator('AXButton', { label: 'Add' }).click()
 *   await window.locator('AXButton', { label: '2' }).click()
 *   await window.locator('AXButton', { label: 'Equals' }).click()
 *   await expect(window.locator('AXStaticText')).toContainText('4')
 * } finally {
 *   await client.close()
 * }
 * ```
 */

export { DriverClient, MCPCallError, DriverTimeoutError, defaultBinaryPath } from './client.js'
export { App, killApp } from './app.js'
export { Window } from './window.js'
export { Locator, type LocatorOptions } from './locator.js'
export { expect, LocatorAssertions } from './expect.js'
