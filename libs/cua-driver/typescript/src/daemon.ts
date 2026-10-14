/**
 * macOS daemon preference for `CuaDriver.create()`.
 *
 * An embedded (in-process) runtime on macOS has no AppKit host, so the agent
 * cursor overlay cannot be drawn and permissions are attributed to the host
 * process. The daemon (CuaDriver.app running `serve`) draws the cursor and owns
 * the TCC grants, so on macOS `create()` connects to it, starting it through
 * LaunchServices when the app is installed. No native dependency, so it is
 * testable without the staged library.
 */
import { spawnSync } from "node:child_process"
import { existsSync } from "node:fs"

const APP_NAME = "CuaDriver"
const APP_PATH = `/Applications/${APP_NAME}.app`
const EMBEDDED_ENV = "CUA_DRIVER_EMBEDDED"

export interface Availability {
  isAvailable(): boolean
}

export interface CreateDriverHooks {
  platform?: NodeJS.Platform
  appInstalled?: () => boolean
  launch?: () => boolean
  timeoutMs?: number
  sleep?: (ms: number) => void
  warn?: (message: string) => void
}

let warned = false

/** Reset the one-time warning (tests only). */
export function resetDaemonWarning(): void {
  warned = false
}

function launchApp(): boolean {
  const result = spawnSync(
    "/usr/bin/open",
    ["-n", "-g", "-a", APP_NAME, "--args", "serve"],
    { stdio: "ignore" },
  )
  return result.status === 0
}

function blockingSleep(ms: number): void {
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, ms)
}

export function createDriver<T extends Availability>(
  connect: () => T,
  createEmbedded: () => T,
  embedded: boolean,
  claudeCodeCompatibility: boolean,
  hooks: CreateDriverHooks = {},
): T {
  const platform = hooks.platform ?? process.platform
  if (
    embedded ||
    claudeCodeCompatibility ||
    platform !== "darwin" ||
    process.env[EMBEDDED_ENV] === "1"
  ) {
    return createEmbedded()
  }

  const driver = connect()
  if (driver.isAvailable()) return driver

  const installed = (hooks.appInstalled ?? (() => existsSync(APP_PATH)))()
  if (installed && (hooks.launch ?? launchApp)()) {
    const sleep = hooks.sleep ?? blockingSleep
    const deadline = Date.now() + (hooks.timeoutMs ?? 15_000)
    while (Date.now() < deadline) {
      if (driver.isAvailable()) return driver
      sleep(250)
    }
  }

  if (!warned) {
    warned = true
    const message =
      "CuaDriver.create() is using the in-process runtime, which cannot draw " +
      "the agent cursor on macOS and attributes permissions to this process. " +
      "Install CuaDriver.app and run `cua-driver serve` (or call " +
      "CuaDriver.connect()) to get the agent cursor, or pass " +
      "{ embedded: true } to silence this warning."
    ;(hooks.warn ?? ((m) => process.emitWarning(m)))(message)
  }
  return createEmbedded()
}
