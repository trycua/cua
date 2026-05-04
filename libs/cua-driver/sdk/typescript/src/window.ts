/**
 * Window — a handle to a specific macOS window (pid + windowId).
 *
 * Obtain from ``App.mainWindow()`` or ``App.getWindow()``.
 */

import type { DriverClient } from './client.js'
import { Locator, type LocatorOptions } from './locator.js'

export class Window {
  constructor(
    private readonly client: DriverClient,
    public readonly pid: number,
    public readonly windowId: number,
  ) {}

  /**
   * Return a {@link Locator} for elements in this window.
   *
   * @param role  AX role to match, e.g. `"AXButton"`, `"AXTextField"`.
   * @param opts  Additional selector constraints (label, title, text, value).
   *
   * @example
   * ```ts
   * window.locator('AXButton', { label: 'OK' }).click()
   * window.locator('AXTextField').nth(1).fill('hello')
   * window.locator(undefined, { text: 'Submit' }).click()
   * ```
   */
  locator(role?: string, opts: Omit<LocatorOptions, 'role' | 'nth'> = {}): Locator {
    return new Locator(this.client, this.pid, this.windowId, { ...opts, role })
  }

  /** Capture the window as a PNG buffer. */
  async screenshot(): Promise<Buffer> {
    const result = await this.client.callTool('screenshot', {
      pid: this.pid,
      window_id: this.windowId,
    })
    const content = result['content'] as Array<{ type: string; data: string }> | undefined
    if (content) {
      for (const item of content) {
        if (item.type === 'image') return Buffer.from(item.data, 'base64')
      }
    }
    throw new Error('No image content in screenshot response')
  }

  /** Return the raw AX tree markdown, optionally filtered by query.
   *
   * ``get_window_state`` returns the AX tree as a text content block —
   * there is no structuredContent.  We scan the content array for the
   * first text item.
   */
  async getTree(query?: string): Promise<string> {
    const params: Record<string, unknown> = { pid: this.pid, window_id: this.windowId }
    if (query) params['query'] = query
    const result = await this.client.callTool('get_window_state', params)
    const content = result['content'] as Array<{ type: string; text?: string }> | undefined
    if (content) {
      for (const item of content) {
        if (item.type === 'text' && item.text != null) return item.text
      }
    }
    return ''
  }

  /** Window title from ``list_windows``. */
  async getTitle(): Promise<string> {
    const result = await this.client.callTool('list_windows', { pid: this.pid })
    const sc = (result['structuredContent'] ?? result) as Record<string, unknown>
    const windows = (sc['windows'] as Array<Record<string, unknown>> | undefined) ?? []
    for (const w of windows) {
      if (w['window_id'] === this.windowId) return (w['title'] as string | undefined) ?? ''
    }
    return ''
  }
}
