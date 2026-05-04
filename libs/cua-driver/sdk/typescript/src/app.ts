/**
 * App — a handle to a running macOS application.
 *
 * Use {@link App.launch} to start an app or construct directly with a known pid.
 *
 * Supports ``Symbol.asyncDispose`` for ``await using`` syntax:
 * ```ts
 * await using app = await App.launch('com.apple.calculator', client)
 * const window = await app.mainWindow()
 * ```
 */

import { execFile, exec } from 'node:child_process'
import type { DriverClient } from './client.js'
import { Window } from './window.js'

// ---------------------------------------------------------------------------
// Standalone helpers
// ---------------------------------------------------------------------------

function execAsync(cmd: string, timeoutMs = 8_000): Promise<{ stdout: string }> {
  return new Promise((resolve) => {
    let done = false
    const finish = (stdout: string) => {
      if (!done) { done = true; resolve({ stdout }) }
    }
    const child = exec(cmd, { timeout: timeoutMs }, (_err, stdout) => finish(stdout ?? ''))
    // Safety net: if exec's own timeout fires but the callback hasn't run yet
    const timer = setTimeout(() => { try { child.kill() } catch {} ; finish('') }, timeoutMs + 500)
    child.on('close', () => clearTimeout(timer))
  })
}

/**
 * Kill all running instances of *bundleId*.
 *
 * Sends a graceful quit event via AppleScript.  With ``force: true``,
 * also sends SIGKILL to any remaining processes via System Events.
 *
 * No {@link DriverClient} required — safe to call before a test suite starts.
 *
 * @example
 * ```ts
 * await killApp('com.apple.calculator')           // graceful quit
 * await killApp('com.apple.calculator', true)     // force kill
 * ```
 */
export async function killApp(bundleId: string, force = false): Promise<void> {
  await execAsync(`osascript -e 'tell application id "${bundleId}" to quit'`).catch(() => {})
  if (force) {
    await sleep(500)
    const { stdout } = await execAsync(
      `osascript -e 'tell application "System Events" to get unix id of ` +
      `(every process whose bundle identifier is "${bundleId}")'`,
    ).catch(() => ({ stdout: '' }))
    for (const pid of stdout.split(/[,\s]+/).map((s) => s.trim())) {
      if (/^\d+$/.test(pid)) {
        await execAsync(`kill -9 ${pid}`).catch(() => {})
      }
    }
  }
}

async function resolveWindowId(client: DriverClient, pid: number): Promise<number> {
  const result = await client.callTool('list_windows', { pid })
  const sc = (result['structuredContent'] ?? result) as Record<string, unknown>
  const windows = (sc['windows'] as Array<Record<string, unknown>> | undefined) ?? []
  if (windows.length === 0) throw new Error(`pid ${pid} has no windows`)

  // Prefer on-screen windows on the current Space
  const preferred = windows.filter(
    (w) => w['is_on_screen'] && w['on_current_space'] !== false,
  )
  const candidates = preferred.length ? preferred : windows
  candidates.sort(
    (a, b) => ((b['z_index'] as number | undefined) ?? 0) - ((a['z_index'] as number | undefined) ?? 0),
  )
  if (candidates[0]?.['window_id'] == null) {
    // Fallback: largest by area
    windows.sort((a, b) => {
      const area = (w: Record<string, unknown>) => {
        const b = (w['bounds'] as Record<string, number> | undefined) ?? {}
        return (b['width'] ?? 0) * (b['height'] ?? 0)
      }
      return area(b) - area(a)
    })
    return windows[0]!['window_id'] as number
  }
  return candidates[0]!['window_id'] as number
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms))
}

function kill(pid: number): Promise<void> {
  return new Promise((resolve) => {
    execFile('kill', [String(pid)], () => resolve())
  })
}

export class App {
  constructor(
    private readonly client: DriverClient,
    public readonly pid: number,
    public readonly bundleId: string,
  ) {}

  /** Launch *bundleId* and return an App handle. */
  static async launch(
    bundleId: string,
    client: DriverClient,
    { settle = 1000 }: { settle?: number } = {},
  ): Promise<App> {
    const result = await client.callTool('launch_app', { bundle_id: bundleId })
    const sc = (result['structuredContent'] ?? result) as Record<string, unknown>
    const pid = sc['pid'] as number
    if (settle > 0) await sleep(settle)
    return new App(client, pid, bundleId)
  }

  /** Return the best available {@link Window} for this app. */
  async mainWindow(timeoutMs = 5_000): Promise<Window> {
    const deadline = Date.now() + timeoutMs
    let lastError: Error = new Error('no windows')
    while (true) {
      try {
        const windowId = await resolveWindowId(this.client, this.pid)
        return new Window(this.client, this.pid, windowId)
      } catch (e) {
        lastError = e as Error
        if (Date.now() >= deadline) throw lastError
        await sleep(300)
      }
    }
  }

  /** Return a {@link Window} whose title contains *title*. */
  async getWindow(title?: string, timeoutMs = 5_000): Promise<Window> {
    const deadline = Date.now() + timeoutMs
    while (true) {
      const result = await this.client.callTool('list_windows', { pid: this.pid })
      const sc = (result['structuredContent'] ?? result) as Record<string, unknown>
      const windows = (sc['windows'] as Array<Record<string, unknown>> | undefined) ?? []
      for (const w of windows) {
        if (title == null || (w['title'] as string | undefined)?.includes(title)) {
          return new Window(this.client, this.pid, w['window_id'] as number)
        }
      }
      if (Date.now() >= deadline) {
        throw new Error(`Window ${title ? `"${title}"` : ''} not found within ${timeoutMs}ms`)
      }
      await sleep(300)
    }
  }

  async quit(): Promise<void> {
    await kill(this.pid)
  }

  async forceQuit(): Promise<void> {
    await new Promise<void>((resolve) => {
      execFile('kill', ['-9', String(this.pid)], () => resolve())
    })
  }

  /** Kill all running instances of *bundleId*. Convenience wrapper around {@link killApp}. */
  static async killByBundleId(bundleId: string, force = false): Promise<void> {
    await killApp(bundleId, force)
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.quit()
  }
}
