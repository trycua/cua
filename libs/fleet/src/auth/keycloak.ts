// Keycloak singleton — initialised once by AuthProvider.
//
// Runtime config comes from window.__CYCLOPS_CS_CFG__, written into
// /config.js by nginx envsubst at container start (see nginx.conf).
// This avoids baking the auth URL into the Vite bundle so a single
// image works across staging/prod/dev.

import Keycloak from "keycloak-js"

declare global {
  interface Window {
    __CYCLOPS_CS_CFG__?: {
      kcUrl?: string
      kcRealm?: string
      kcClientId?: string
    }
  }
}

const cfg = (typeof window !== "undefined" && window.__CYCLOPS_CS_CFG__) || {}

export const kc = new Keycloak({
  url: cfg.kcUrl ?? "https://auth.cua.ai",
  realm: cfg.kcRealm ?? "cyclops-cs",
  clientId: cfg.kcClientId ?? "cyclops-cs-spa",
})

let initialised = false

export async function initKc(): Promise<boolean> {
  if (initialised) return kc.authenticated ?? false
  initialised = true
  const authed = await kc.init({
    onLoad: "login-required",
    pkceMethod: "S256",
    checkLoginIframe: false,
  })
  // Refresh proactively when the token is about to expire — keycloak-js
  // fires onTokenExpired *after* expiry, which would race the next API
  // call. updateToken(30) refreshes if <30s remain.
  kc.onTokenExpired = () => {
    kc.updateToken(30).catch(() => kc.login())
  }
  return authed
}

// getToken returns a fresh access token, refreshing it if it's within
// 30s of expiry. Used by the API client's fetchJson injector.
export async function getToken(): Promise<string | undefined> {
  if (!kc.authenticated) return undefined
  try {
    await kc.updateToken(30)
  } catch {
    kc.login()
    return undefined
  }
  return kc.token
}

export function logout() {
  kc.logout()
}

export function userInfo(): { sub?: string; email?: string; name?: string } {
  const t = kc.tokenParsed as Record<string, unknown> | undefined
  if (!t) return {}
  return {
    sub: t.sub as string | undefined,
    email: t.email as string | undefined,
    name: (t.preferred_username ?? t.name) as string | undefined,
  }
}
