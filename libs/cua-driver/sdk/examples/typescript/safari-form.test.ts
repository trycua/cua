/**
 * Safari form-fill tests — TypeScript async, Vitest.
 *
 * Runs CONCURRENTLY with calculator.test.ts (second parallel fork).
 * Opens ``fixtures/form_all_inputs.html`` in Safari and interacts with
 * each input type via the cua-driver SDK.
 *
 * Requirements:
 *   Safari › Develop › Allow JavaScript from Apple Events must be ON.
 *
 * Run just this file:
 *   npx vitest run safari-form.test.ts
 */

import { beforeAll, afterAll, describe, it, expect as vitestExpect } from 'vitest'
import { execFile, exec } from 'node:child_process'
import { promisify } from 'node:util'
import * as path from 'node:path'
import { fileURLToPath } from 'node:url'

import { DriverClient, App, expect as driverExpect, killApp } from '@cua/driver-sdk'
import type { Window } from '@cua/driver-sdk'

const execFileAsync = promisify(execFile)  // used for open -g
const execAsync    = promisify(exec)

const __dirname = path.dirname(fileURLToPath(import.meta.url))
const FORM_HTML = path.resolve(
  __dirname, '..', '..', '..', 'Tests', 'integration', 'fixtures', 'form_all_inputs.html',
)
const FORM_URL  = `file://${FORM_HTML}`
const SAFARI_BUNDLE = 'com.apple.Safari'

// ---------------------------------------------------------------------------
// JS helpers (Safari Develop › Allow JS from Apple Events required)
// ---------------------------------------------------------------------------

async function js(expr: string): Promise<string> {
  const script = `tell application "Safari" to do JavaScript "${expr}" in front document`
  const { stdout } = await execAsync(`osascript -e '${script}'`)
  return stdout.trim()
}

async function waitForForm(timeoutMs = 20_000): Promise<boolean> {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    if (await js('typeof getFieldValues').catch(() => '') === 'function') return true
    await new Promise((r) => setTimeout(r, 500))
  }
  return false
}

async function fieldValue(name: string): Promise<string> {
  const raw = await js('getFieldValues()')
  try {
    const obj = JSON.parse(raw) as Record<string, unknown>
    return String(obj[name] ?? '')
  } catch {
    return ''
  }
}

async function checkboxChecked(): Promise<boolean> {
  const raw = await js('getFieldValues()')
  try {
    const obj = JSON.parse(raw) as Record<string, unknown>
    return Boolean(obj['checkbox'])
  } catch {
    return false
  }
}

// ---------------------------------------------------------------------------
// Suite setup
// ---------------------------------------------------------------------------

let client: DriverClient
let app: App
let window: Window

beforeAll(async () => {
  await killApp(SAFARI_BUNDLE, true)
  await new Promise((r) => setTimeout(r, 2000))

  // Open Safari to the form in background (-g)
  await execFileAsync('open', ['-g', '-a', 'Safari', FORM_URL])
  const loaded = await waitForForm(25_000)
  vitestExpect(loaded, `Safari did not load ${FORM_HTML} within 25s\n` +
    'Tip: Safari › Develop › Allow JavaScript from Apple Events').toBe(true)

  // Connect driver to Safari.
  // list_apps returns text content (no structuredContent).
  // Format: "- AppName (pid N) [bundle.id]"
  client = await DriverClient.create()
  const appsResult = await client.callTool('list_apps')
  const content = appsResult['content'] as Array<{ type: string; text?: string }> | undefined
  let appsText = ''
  if (content) {
    for (const item of content) {
      if (item.type === 'text' && item.text) { appsText = item.text; break }
    }
  }
  let safariPid: number | undefined
  for (const line of appsText.split('\n')) {
    if (line.includes(SAFARI_BUNDLE)) {
      const m = line.match(/\(pid (\d+)\)/)
      if (m) { safariPid = parseInt(m[1]!, 10); break }
    }
  }
  vitestExpect(safariPid, `Safari not found in app list.\nlist_apps output:\n${appsText}`).toBeDefined()

  app = new App(client, safariPid!, SAFARI_BUNDLE)
  window = await app.mainWindow()
}, 40_000)

afterAll(async () => {
  await client?.close()
  // Graceful quit in teardown — don't force-kill as it triggers macOS restore dialog
  await killApp(SAFARI_BUNDLE)
})

// ---------------------------------------------------------------------------
// Tests — one per form input type
// ---------------------------------------------------------------------------

describe('Safari form fill', () => {
  it('fills text input', async () => {
    await window.locator('AXTextField', { label: 'Text' }).type('Hello World')
    await new Promise((r) => setTimeout(r, 500))
    vitestExpect(await fieldValue('text')).toBe('Hello World')
  })

  it('fills password input', async () => {
    await window.locator('AXTextField', { label: 'Password' }).type('Pass1234!')
    await new Promise((r) => setTimeout(r, 500))
    vitestExpect(await fieldValue('password')).toBe('Pass1234!')
  })

  it('fills email input', async () => {
    await window.locator('AXTextField', { label: 'Email' }).type('agent@example.com')
    await new Promise((r) => setTimeout(r, 500))
    vitestExpect(await fieldValue('email')).toBe('agent@example.com')
  })

  it('fills number input', async () => {
    await window.locator('AXTextField', { label: 'Number' }).type('42')
    await new Promise((r) => setTimeout(r, 500))
    vitestExpect(await fieldValue('number')).toBe('42')
  })

  it('fills textarea', async () => {
    await window.locator('AXTextArea', { label: 'Message (Textarea)' }).type('ts works')
    await new Promise((r) => setTimeout(r, 500))
    vitestExpect(await fieldValue('textarea')).toContain('ts works')
  })

  it('checks checkbox', async () => {
    await window.locator('AXCheckBox', { label: 'I agree to the terms' }).click()
    await new Promise((r) => setTimeout(r, 300))
    vitestExpect(await checkboxChecked()).toBe(true)
  })

  it('selects dropdown option', async () => {
    await window.locator('AXPopUpButton', { label: 'Favourite colour (Select)' }).setValue('Blue')
    await new Promise((r) => setTimeout(r, 300))
    vitestExpect(await fieldValue('select')).toBe('blue')
  })

  it('selects radio button', async () => {
    await window.locator('AXRadioButton', { label: 'Banana' }).click()
    await new Promise((r) => setTimeout(r, 300))
    vitestExpect(await fieldValue('radio')).toBe('banana')
  })

  it('submits form', async () => {
    await window.locator('AXButton', { label: 'Submit Form' }).click()
    await new Promise((r) => setTimeout(r, 500))
    const submitted = await js('(window._submitted !== undefined).toString()')
    vitestExpect(submitted).toBe('true')
  })
})
