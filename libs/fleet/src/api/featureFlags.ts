// Feature flags client — fetches /api/config to get per-user flags.
// The backend derives admin status from the OpenFeature flag
// `/feature-flags/cyclops-cs/admin-subs` (AWS SSM Parameter Store in prod,
// or the CYCLOPS_CS_ADMIN_SUBS env var in dev) — a JSON array of Keycloak
// sub UUIDs. Access to infra-only resources is also enforced server-side by
// the backend's OPA policy, not just by hiding nav here.

import { getToken } from "../auth/keycloak"

export interface FeatureFlags {
  /** Admin users see the full infra UI: Nodes and Operator events. */
  admin: boolean
}

const DEFAULT_FLAGS: FeatureFlags = {
  admin: false,
}

let _cache: FeatureFlags | null = null
let _fetchPromise: Promise<FeatureFlags> | null = null

export async function fetchFeatureFlags(): Promise<FeatureFlags> {
  if (_cache) return _cache
  if (_fetchPromise) return _fetchPromise

  _fetchPromise = (async () => {
    try {
      const token = await getToken()
      const res = await fetch("/api/config", {
        headers: {
          Authorization: token ? `Bearer ${token}` : "",
          "Content-Type": "application/json",
        },
      })
      if (!res.ok) return DEFAULT_FLAGS
      const data = (await res.json()) as FeatureFlags
      _cache = {
        admin: data.admin ?? false,
      }
      return _cache
    } catch {
      return DEFAULT_FLAGS
    } finally {
      _fetchPromise = null
    }
  })()

  return _fetchPromise
}

/** Invalidate the cache (e.g., after login). */
export function invalidateFeatureFlags(): void {
  _cache = null
}
