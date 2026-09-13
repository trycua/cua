const oidcTerminalParameters = [
  "code",
  "error",
] as const

const oidcCallbackParameters = [
  "code",
  "error",
  "error_description",
  "error_uri",
  "iss",
  "session_state",
  "state",
] as const

function removeOidcCallback(parameters: URLSearchParams): boolean {
  const isCallback =
    parameters.has("state") &&
    oidcTerminalParameters.some(parameter => parameters.has(parameter))
  if (!isCallback) return false

  for (const parameter of oidcCallbackParameters) {
    parameters.delete(parameter)
  }
  return true
}

/**
 * Return a same-origin dashboard URL that is safe to use for a fresh login.
 *
 * Keycloak normally removes callback parameters after processing them. This
 * extra cleanup prevents a failed refresh or rejected callback from sending a
 * stale code/state pair through another authorization loop. Unrelated search
 * parameters and fragments remain intact.
 */
export function appLoginRedirectUri(currentHref: string): string {
  const current = new URL(currentHref)
  removeOidcCallback(current.searchParams)

  if (current.hash.length > 1) {
    const hashParameters = new URLSearchParams(current.hash.slice(1))
    if (removeOidcCallback(hashParameters)) {
      const nextHash = hashParameters.toString()
      current.hash = nextHash ? `#${nextHash}` : ""
    }
  }

  return current.toString()
}

/** Sign out to a stable allowlisted Run entry point, never caller input. */
export function appLogoutRedirectUri(currentHref: string): string {
  return new URL("/", currentHref).toString()
}
