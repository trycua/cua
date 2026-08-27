interface SessionStorageLike {
  getItem(key: string): string | null
  setItem(key: string, value: string): void
}

interface AnalyticsDependencies {
  getToken: () => Promise<string | undefined>
  fetch: (input: string, init?: RequestInit) => Promise<Response>
  storage: SessionStorageLike
}

const storagePrefix = "fleet-login-recorded:"

function defaultDependencies(): AnalyticsDependencies {
  return {
    getToken: async () => {
      const { getToken } = await import("./keycloak")
      return getToken()
    },
    fetch: (input, init) => fetch(input, init),
    storage: window.sessionStorage,
  }
}

export async function recordFleetLogin(
  sessionID: string | undefined,
  dependencies: AnalyticsDependencies = defaultDependencies(),
): Promise<void> {
  const marker = `${storagePrefix}${sessionID || "authenticated"}`
  if (dependencies.storage.getItem(marker) === "true") return

  const token = await dependencies.getToken()
  if (!token) throw new Error("Authentication token is unavailable")
  const response = await dependencies.fetch("/api/analytics/session", {
    method: "POST",
    headers: { Authorization: `Bearer ${token}` },
  })
  if (!response.ok) {
    throw new Error(`Fleet login analytics request failed: ${response.status}`)
  }
  dependencies.storage.setItem(marker, "true")
}
