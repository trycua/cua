// Keycloak singleton — initialised once by AuthProvider.
//
// Runtime config comes from window.__CYCLOPS_CS_CFG__, written into
// /config.js by nginx envsubst at container start (see nginx.conf).
// This avoids baking the auth URL into the Vite bundle so a single
// image works across staging/prod/dev.

import Keycloak from "keycloak-js"
import { isLocalVisualPreview } from "../local-visual-preview"
import {
  appLoginRedirectUri,
  appLogoutRedirectUri,
} from "./redirects"

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

let initialisation: Promise<boolean> | null = null
let reauthentication: Promise<void> | null = null

function currentHref(): string {
  return window.location.href
}

function reauthenticate(): Promise<void> {
  if (reauthentication) return reauthentication

  kc.clearToken()
  reauthentication = kc
    .login({ redirectUri: appLoginRedirectUri(currentHref()) })
    .catch(error => {
      reauthentication = null
      throw error
    })
  return reauthentication
}

export function initKc(): Promise<boolean> {
  if (initialisation) return initialisation

  initialisation = kc
    .init({
      onLoad: "login-required",
      flow: "standard",
      pkceMethod: "S256",
      checkLoginIframe: false,
    })
    .then(authed => {
      // Refresh proactively when the token is about to expire — keycloak-js
      // fires onTokenExpired *after* expiry, which would race the next API
      // call. updateToken(30) refreshes if <30s remain.
      kc.onTokenExpired = () => {
        kc.updateToken(30).catch(() => reauthenticate())
      }
      return authed
    })
    .catch(error => {
      initialisation = null
      throw error
    })

  return initialisation
}

// getToken returns a fresh access token, refreshing it if it's within
// 30s of expiry. Used by the API client's fetchJson injector.
export async function getToken(): Promise<string | undefined> {
  if (!kc.authenticated) return undefined
  try {
    await kc.updateToken(30)
  } catch {
    await reauthenticate()
    return undefined
  }
  return kc.token
}

export function logout(): Promise<void> {
  return kc.logout({ redirectUri: appLogoutRedirectUri(currentHref()) })
}

export function userInfo(): { sub?: string; email?: string; name?: string } {
  if (isLocalVisualPreview()) {
    return { sub: "local-preview", email: "preview@cua.local", name: "Preview" }
  }
  const t = kc.tokenParsed as Record<string, unknown> | undefined
  if (!t) return {}
  return {
    sub: t.sub as string | undefined,
    email: t.email as string | undefined,
    name: (t.preferred_username ?? t.name) as string | undefined,
  }
}
