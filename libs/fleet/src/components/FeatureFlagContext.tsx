import { createContext, useContext, useEffect, useState, type ReactNode } from "react"
import { fetchFeatureFlags, type FeatureFlags } from "../sdk/featureFlags"

const DEFAULT_FLAGS: FeatureFlags = { admin: false, billing: false }

interface FeatureFlagContextValue extends FeatureFlags {
  resolved: boolean
}

const FeatureFlagContext = createContext<FeatureFlagContextValue>({
  ...DEFAULT_FLAGS,
  resolved: false,
})

/**
 * Fetches /api/config once at app startup and provides the result via
 * context. All children can call useFeatureFlags() to read flags.
 */
export function FeatureFlagProvider({ children }: { children: ReactNode }) {
  const [value, setValue] = useState<FeatureFlagContextValue>({
    ...DEFAULT_FLAGS,
    resolved: false,
  })

  useEffect(() => {
    fetchFeatureFlags()
      .then(flags => setValue({ ...flags, resolved: true }))
      .catch(() => setValue({ ...DEFAULT_FLAGS, resolved: true }))
  }, [])

  return (
    <FeatureFlagContext.Provider value={value}>
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
export function useFeatureFlags(): FeatureFlagContextValue {
  return useContext(FeatureFlagContext)
}
