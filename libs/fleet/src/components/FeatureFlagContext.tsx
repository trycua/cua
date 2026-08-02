import { createContext, useContext, useEffect, useState, type ReactNode } from "react"
import { fetchFeatureFlags, type FeatureFlags } from "../api/featureFlags"

const DEFAULT_FLAGS: FeatureFlags = { admin: false }

const FeatureFlagContext = createContext<FeatureFlags>(DEFAULT_FLAGS)

/**
 * Fetches /api/config once at app startup and provides the result via
 * context. All children can call useFeatureFlags() to read flags.
 */
export function FeatureFlagProvider({ children }: { children: ReactNode }) {
  const [flags, setFlags] = useState<FeatureFlags>(DEFAULT_FLAGS)

  useEffect(() => {
    fetchFeatureFlags().then(setFlags).catch(() => setFlags(DEFAULT_FLAGS))
  }, [])

  return (
    <FeatureFlagContext.Provider value={flags}>
      {children}
    </FeatureFlagContext.Provider>
  )
}

/**
 * Returns the current user's feature flags.
 *
 * @example
 * const { admin } = useFeatureFlags()
 * if (admin) { ... }
 */
export function useFeatureFlags(): FeatureFlags {
  return useContext(FeatureFlagContext)
}
