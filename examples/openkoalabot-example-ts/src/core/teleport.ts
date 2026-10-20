/**
 * What this sample says about teleport on the MIT SDK.
 *
 * Session teleport (`space.teleportManifest` / `space.teleport`) is a tool
 * call. It works when the SDK is connected to the Cua Spaces daemon
 * (`--daemon`); an embedded runtime refuses it with
 * `HostCapabilityMissing`, whose message says teleport ships with Cua
 * Spaces. "Teleport an app…" (the app catalog, plan and run, window drags)
 * ships only with Cua Spaces, so this sample shows a message instead.
 */

/** Shown where the sample used to offer "Teleport an app…". */
export const APP_TELEPORT_MESSAGE = "App teleport ships with Cua Spaces (source-available)."

/**
 * The reason, when `error` is the SDK refusing teleport because this runtime
 * has no Cua Spaces (a `HostCapabilityMissing` whose message says teleport
 * ships with Cua Spaces); `undefined` for any other error.
 */
export function teleportNeedsCuaSpaces(error: unknown): string | undefined {
  const e = error as { tag?: unknown; code?: unknown; cause?: { tag?: unknown } } | null
  const message = error instanceof Error ? error.message : String(error)
  const tagged = e?.tag === "HostCapabilityMissing" || e?.cause?.tag === "HostCapabilityMissing" || e?.code === "host_capability_missing"
  if (!/ships with Cua Spaces/i.test(message)) return undefined
  if (!tagged && !/HostCapabilityMissing|not available on this host/i.test(message)) return undefined
  return message.replace(/^Error:\s*/, "")
}
