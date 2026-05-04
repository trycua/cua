/**
 * Locator — auto-waiting element selector for cua-driver windows.
 *
 * Mirrors the Playwright Locator API.  Obtain from ``Window.locator()``.
 * All action and query methods wait up to ``timeout`` ms for the element
 * to appear in the AX tree before interacting.
 */

import type { DriverClient } from './client.js'

const DEFAULT_TIMEOUT = 5_000  // ms
const POLL_INTERVAL  = 300     // ms

// ---------------------------------------------------------------------------
// AX tree parsing helpers
// ---------------------------------------------------------------------------

function extractElementIndex(line: string): number | null {
  const m = line.match(/\[(\d+)\]/)
  return m ? parseInt(m[1]!, 10) : null
}

export interface LocatorOptions {
  role?: string
  label?: string
  title?: string
  text?: string
  value?: string
  nth?: number
}

function matchesLine(line: string, opts: LocatorOptions): boolean {
  if (opts.role && !line.includes(opts.role)) return false
  if (opts.label) {
    const lb = opts.label
    // Match the parenthesised description, id=, help=, or quoted title.
    // The loose substring fallback is omitted — digit labels like "2" would
    // otherwise match "[2]" in any element's index bracket.
    if (
      !line.includes(`(${lb})`) &&
      !line.includes(`id=${lb}`) &&
      !line.includes(`help="${lb}`) &&
      !line.includes(`"${lb}"`)
    ) return false
  }
  if (opts.title && !line.includes(`"${opts.title}"`)) return false
  if (opts.text && !line.includes(opts.text)) return false
  if (opts.value) {
    const v = opts.value
    if (!line.includes(`value="${v}"`) && !line.includes(`value="${v.toLowerCase()}"`))
      return false
  }
  return true
}

function findInTree(tree: string, opts: LocatorOptions): number | null {
  const nth = opts.nth ?? 0
  const matches: number[] = []
  for (const line of tree.split('\n')) {
    if (!line.trim() || !line.includes('[')) continue
    const idx = extractElementIndex(line)
    if (idx !== null && matchesLine(line, opts)) {
      matches.push(idx)
    }
  }
  return nth < matches.length ? matches[nth]! : null
}

function textContentFromTree(tree: string, elementIndex: number): string {
  for (const line of tree.split('\n')) {
    const m = line.match(/\[(\d+)\]/)
    if (m && parseInt(m[1]!, 10) === elementIndex) {
      const title = line.match(/"([^"]*)"/)
      if (title) return title[1]!
      const val = line.match(/value="([^"]*)"/)
      if (val) return val[1]!
      const label = line.match(/\(([^)]+)\)/)
      if (label) return label[1]!
    }
  }
  return ''
}

/**
 * Extract text from non-indexed AX tree lines, e.g. ``- AXStaticText = "value"``.
 * These appear for display labels (Calculator result, etc.) that have no element index.
 */
function extractUnindexedText(tree: string, opts: LocatorOptions, nth: number): string | null {
  let count = 0
  for (const line of tree.split('\n')) {
    if (!line.trim() || /\[\d+\]/.test(line)) continue  // skip blank / indexed
    if (!matchesLine(line, opts)) continue
    const eq = line.match(/=\s*"([^"]*)"/)
    if (eq) {
      if (count === nth) return eq[1]!
      count++
    }
  }
  return null
}

/**
 * Collect text from EVERY matching line (indexed and non-indexed).
 * Used by toContainText/toHaveText to check across multiple dynamic elements.
 */
function allTextFromTree(tree: string, opts: LocatorOptions): string[] {
  const texts: string[] = []
  // Indexed elements
  for (const line of tree.split('\n')) {
    if (!line.trim() || !line.includes('[')) continue
    const idx = extractElementIndex(line)
    if (idx !== null && matchesLine(line, opts)) {
      const title = line.match(/"([^"]*)"/)
      if (title) { texts.push(title[1]!); continue }
      const val = line.match(/value="([^"]*)"/)
      if (val) { texts.push(val[1]!); continue }
    }
  }
  // Non-indexed elements
  for (const line of tree.split('\n')) {
    if (!line.trim() || /\[\d+\]/.test(line)) continue
    if (!matchesLine(line, opts)) continue
    const eq = line.match(/=\s*"([^"]*)"/)
    if (eq) texts.push(eq[1]!)
  }
  return texts
}

function valueFromTree(tree: string, elementIndex: number): string {
  for (const line of tree.split('\n')) {
    const m = line.match(/\[(\d+)\]/)
    if (m && parseInt(m[1]!, 10) === elementIndex) {
      const val = line.match(/value="([^"]*)"/)
      return val ? val[1]! : ''
    }
  }
  return ''
}

// ---------------------------------------------------------------------------
// Locator
// ---------------------------------------------------------------------------

export class Locator {
  constructor(
    private readonly client: DriverClient,
    private readonly pid: number,
    private readonly windowId: number,
    private readonly opts: LocatorOptions,
  ) {}

  /** Return a new Locator for the *n*-th match (0-based). */
  nth(n: number): Locator {
    return new Locator(this.client, this.pid, this.windowId, { ...this.opts, nth: n })
  }

  // ------------------------------------------------------------------
  // Actions
  // ------------------------------------------------------------------

  async click(timeoutMs = DEFAULT_TIMEOUT): Promise<void> {
    const [idx] = await this.waitForElement(timeoutMs)
    await this.client.callTool('click', {
      pid: this.pid,
      window_id: this.windowId,
      element_index: idx,
    })
  }

  async fill(text: string, timeoutMs = DEFAULT_TIMEOUT): Promise<void> {
    const [idx] = await this.waitForElement(timeoutMs)
    await this.client.callTool('click', { pid: this.pid, window_id: this.windowId, element_index: idx })
    await sleep(100)
    await this.client.callTool('type_text', { pid: this.pid, text })
  }

  async type(text: string, timeoutMs = DEFAULT_TIMEOUT): Promise<void> {
    const [idx] = await this.waitForElement(timeoutMs)
    await this.client.callTool('click', { pid: this.pid, window_id: this.windowId, element_index: idx })
    await sleep(100)
    await this.client.callTool('type_text_chars', { pid: this.pid, text })
  }

  async setValue(value: string, timeoutMs = DEFAULT_TIMEOUT): Promise<void> {
    const [idx] = await this.waitForElement(timeoutMs)
    await this.client.callTool('set_value', {
      pid: this.pid,
      window_id: this.windowId,
      element_index: idx,
      value,
    })
  }

  async doubleClick(timeoutMs = DEFAULT_TIMEOUT): Promise<void> {
    const [idx] = await this.waitForElement(timeoutMs)
    await this.client.callTool('double_click', {
      pid: this.pid,
      window_id: this.windowId,
      element_index: idx,
    })
  }

  // ------------------------------------------------------------------
  // Queries
  // ------------------------------------------------------------------

  async textContent(timeoutMs = DEFAULT_TIMEOUT): Promise<string> {
    const query = this.buildQuery()
    const deadline = Date.now() + timeoutMs
    while (true) {
      const tree = await this.getTree(query)

      // Try indexed element first
      const idx = findInTree(tree, this.opts)
      if (idx !== null) {
        return textContentFromTree(tree, idx)
      }

      // Fallback: non-indexed elements (e.g. "- AXStaticText = \"value\"")
      const text = extractUnindexedText(tree, this.opts, this.opts.nth ?? 0)
      if (text !== null) return text

      if (Date.now() >= deadline) {
        throw new Error(
          `Element text not found within ${timeoutMs}ms: ` +
          `role=${this.opts.role} label=${this.opts.label}`,
        )
      }
      await sleep(POLL_INTERVAL)
    }
  }

  /** Return text from ALL matching elements (indexed + non-indexed).
   *
   * Used by {@link LocatorAssertions.toContainText} to check across multiple
   * dynamic elements (e.g. Calculator result + expression-history displays).
   */
  async allTextContents(query?: string): Promise<string[]> {
    const tree = await this.getTree(query ?? this.buildQuery())
    return allTextFromTree(tree, this.opts)
  }

  async getValue(timeoutMs = DEFAULT_TIMEOUT): Promise<string> {
    const [idx, tree] = await this.waitForElement(timeoutMs)
    return valueFromTree(tree, idx)
  }

  async isVisible(timeoutMs = 0): Promise<boolean> {
    try {
      await this.waitForElement(timeoutMs)
      return true
    } catch {
      return false
    }
  }

  // ------------------------------------------------------------------
  // Internal
  // ------------------------------------------------------------------

  buildQuery(): string | undefined {
    // Only pass the role as a cua-driver query hint.  Multi-word queries
    // (e.g. "AXButton 2") can return empty results when the label is
    // encoded differently in the tree (e.g. "(2)" vs "2").
    return this.opts.role ?? undefined
  }

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

  async waitForElement(timeoutMs: number): Promise<[number, string]> {
    const query = this.buildQuery()
    const deadline = Date.now() + timeoutMs
    while (true) {
      const tree = await this.getTree(query)
      const idx = findInTree(tree, this.opts)
      if (idx !== null) return [idx, tree]
      if (Date.now() >= deadline) {
        throw new Error(
          `Element not found within ${timeoutMs}ms: ` +
          `role=${this.opts.role} label=${this.opts.label} ` +
          `title=${this.opts.title} text=${this.opts.text}`,
        )
      }
      await sleep(POLL_INTERVAL)
    }
  }
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms))
}
