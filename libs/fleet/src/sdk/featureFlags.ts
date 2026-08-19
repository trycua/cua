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
  /** Billing navigation and page are disabled by default. */
  billing: boolean
  /** Chat users can access persisted browser-bash conversations. */
  chat: boolean
  /** Internal users can access the preview usage dashboard. */
  usage: boolean
}

let cache: FeatureFlags | null = null
let generation = 0
let fetchPromise: { generation: number; promise: Promise<FeatureFlags> } | null = null

export async function fetchFeatureFlags(
  options: { force?: boolean } = {},
): Promise<FeatureFlags> {
  if (options.force) {
    cache = null
    generation += 1
  }
  if (cache) return cache
  if (fetchPromise?.generation === generation) return fetchPromise.promise

  const requestGeneration = generation
  const promise = (async () => {
    const token = await getToken()
    const response = await fetch("/api/config", {
      headers: {
        Authorization: token ? `Bearer ${token}` : "",
        "Content-Type": "application/json",
      },
    })
    if (!response.ok) {
      throw new Error(`feature flag config request failed: ${response.status}`)
    }

    const data = (await response.json()) as Partial<FeatureFlags>
    if (!data || typeof data !== "object") {
      throw new Error("feature flag config response must be an object")
    }
    const flags = {
      admin: data.admin ?? false,
      billing: data.billing ?? false,
      chat: data.chat ?? false,
      usage: data.usage ?? false,
    }
    if (
      typeof flags.admin !== "boolean" ||
      typeof flags.billing !== "boolean" ||
      typeof flags.chat !== "boolean" ||
      typeof flags.usage !== "boolean"
    ) {
      throw new Error("feature flag config response contains invalid values")
    }
    if (requestGeneration === generation) {
      cache = flags
    }
    return flags
  })()

  fetchPromise = { generation: requestGeneration, promise }
  try {
    return await promise
  } finally {
    if (fetchPromise?.generation === requestGeneration) {
      fetchPromise = null
    }
  }
}

/** Invalidate the cache (e.g., after login). */
export function invalidateFeatureFlags(): void {
  cache = null
  generation += 1
}
