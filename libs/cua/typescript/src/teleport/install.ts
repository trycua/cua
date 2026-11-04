/**
 * The "Install Cua" affordance for a teleport the Keyvault refused.
 *
 * Moving a signed-in session goes through the Cua Keyvault, which only the
 * installed, signed Cua app hosts. An app that embeds the SDK is refused by
 * design with the `requires_cua_app` code; this turns that refusal into a
 * prompt a UI can show (title, one line, an install button) instead of the
 * raw error. The refusal itself is unchanged: nothing here retries or
 * bypasses it.
 */

/** Where people install Cua (the CLI and the Cua app). */
export const CUA_INSTALL_URL = "https://cua.ai/install"
/** Opens the Keyvault page of an installed Cua app. */
export const CUA_OPEN_URL = "cua://keyvault"

export interface InstallCuaPrompt {
  /** Cua is installed but not running (open it instead of installing). */
  installed: boolean
  title: string
  message: string
  /** The button label. */
  actionLabel: string
  /** The button target: always one of the constants above, never a URL
   * taken from the error, so a remote message cannot supply the link. */
  url: string
}

const CODE = /\brequires_cua_app\b|\bRequiresCuaApp\b/
const NOT_RUNNING = /installed but not running|needs the Cua app running/i

function texts(error: unknown, depth = 0): string[] {
  if (error == null || depth > 3) return []
  if (typeof error === "string") return [error]
  if (typeof error !== "object") return [String(error)]
  const o = error as Record<string, unknown>
  const out: string[] = []
  for (const key of ["code", "kind", "tag", "message", "detail"]) {
    if (typeof o[key] === "string") out.push(o[key] as string)
  }
  if (o.error !== undefined) out.push(...texts(o.error, depth + 1))
  if (o.cause !== undefined) out.push(...texts(o.cause, depth + 1))
  if (typeof o.installed === "boolean" && typeof o.open_url === "string" && o.open_url === CUA_OPEN_URL) {
    out.push("requires_cua_app")
  }
  return out
}

/**
 * The prompt when `error` (an `Error`, a message, or a tool result such as
 * `{"error":{"code":"requires_cua_app"}}`) is the Keyvault's "needs the Cua
 * app" refusal; `null` for any other failure.
 */
export function requiresCuaApp(error: unknown): InstallCuaPrompt | null {
  const all = texts(error)
  if (!all.some((t) => CODE.test(t))) return null
  const installed =
    all.some((t) => NOT_RUNNING.test(t)) ||
    (typeof error === "object" && error !== null && (error as { installed?: unknown }).installed === true)
  return installed
    ? {
        installed: true,
        title: "Open Cua to teleport your session",
        message: "The Cua app keeps your logins in its Keyvault and asks you before sharing them. Open it, then try again.",
        actionLabel: "Open Cua",
        url: CUA_OPEN_URL,
      }
    : {
        installed: false,
        title: "Install Cua to teleport your session",
        message: "The Cua app keeps your logins in its Keyvault and asks you before sharing them.",
        actionLabel: "Install Cua",
        url: CUA_INSTALL_URL,
      }
}
