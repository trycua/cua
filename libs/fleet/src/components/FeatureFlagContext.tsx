import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react"
import { fetchFeatureFlags, type FeatureFlags } from "../api/featureFlags"
import { isLocalVisualPreview } from "../local-visual-preview"
import { DEFAULT_USAGE_PRICING } from "../usagePricing"

const DEFAULT_FLAGS: FeatureFlags = {
  admin: false,
  billing: false,
  chat: false,
  usage: false,
  usagePricing: DEFAULT_USAGE_PRICING,
}

interface FeatureFlagContextValue extends FeatureFlags {
  resolved: boolean
  refresh: () => Promise<FeatureFlags>
}

const unavailableRefresh = async (): Promise<FeatureFlags> => DEFAULT_FLAGS

const FeatureFlagContext = createContext<FeatureFlagContextValue>({
  ...DEFAULT_FLAGS,
  resolved: false,
  refresh: unavailableRefresh,
})

/**
 * Fetches /api/config once at app startup and provides the result via
 * context. All children can call useFeatureFlags() to read flags.
 */
export function FeatureFlagProvider({ children }: { children: ReactNode }) {
  const visualPreview = isLocalVisualPreview()
  const [value, setValue] = useState<Omit<FeatureFlagContextValue, "refresh">>({
    ...(visualPreview
      ? { ...DEFAULT_FLAGS, admin: true, billing: true, chat: true, usage: true }
      : DEFAULT_FLAGS),
    resolved: visualPreview,
  })
  const requestGeneration = useRef(0)

  const refresh = useCallback(async (): Promise<FeatureFlags> => {
    const currentGeneration = ++requestGeneration.current
    setValue({ ...DEFAULT_FLAGS, resolved: false })
    try {
      const flags = await fetchFeatureFlags({ force: true })
      if (requestGeneration.current === currentGeneration) {
        setValue({ ...flags, resolved: true })
      }
      return flags
    } catch (error) {
      if (requestGeneration.current === currentGeneration) {
        setValue({ ...DEFAULT_FLAGS, resolved: true })
      }
      throw error
    }
  }, [])

  useEffect(() => {
    if (visualPreview) return
    const currentGeneration = ++requestGeneration.current
    fetchFeatureFlags()
      .then(flags => {
        if (requestGeneration.current === currentGeneration) {
          setValue({ ...flags, resolved: true })
        }
      })
      .catch(() => {
        if (requestGeneration.current === currentGeneration) {
          setValue({ ...DEFAULT_FLAGS, resolved: true })
        }
      })
  }, [visualPreview])

  return (
    <FeatureFlagContext.Provider value={{ ...value, refresh }}>
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
