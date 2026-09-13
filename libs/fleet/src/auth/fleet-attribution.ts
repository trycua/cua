export const ATTRIBUTION_STORAGE_KEY = "cua.fleet-attribution.v1"
export const ATTRIBUTION_TTL_MS = 7 * 24 * 60 * 60 * 1000
export const ATTRIBUTION_MAX_VALUE_LENGTH = 128
export const ATTRIBUTION_MAX_RECORD_BYTES = 2048

const allowedKeys = ["campaign_id", "content_id", "utm_source", "utm_medium", "utm_campaign"] as const
type AttributionKey = (typeof allowedKeys)[number]
export type AttributionRecord = { version: 1; capturedAt: number; values: Partial<Record<AttributionKey, string>> }
export interface StorageLike { getItem(key: string): string | null; setItem(key: string, value: string): void; removeItem?(key: string): void }
export interface AttributionBinder { bind(record: AttributionRecord, accessToken: string): Promise<void> | void }
type Options = { storage?: StorageLike; now?: () => number; getAccessToken?: () => Promise<string | undefined> }

const storageDefault = (): StorageLike | undefined => typeof window === "undefined" ? undefined : window.sessionStorage
const validValue = (value: string) => value.length > 0 && value.length <= ATTRIBUTION_MAX_VALUE_LENGTH && /^[A-Za-z0-9._~-]+$/.test(value)
const encode = (record: AttributionRecord) => JSON.stringify(record)

export function captureFleetAttribution(href: string, options: Options = {}): void {
  const storage = options.storage ?? storageDefault()
  if (!storage || storage.getItem(ATTRIBUTION_STORAGE_KEY) !== null) return
  let params: URLSearchParams
  try { params = new URL(href).searchParams } catch { return }
  const values: Partial<Record<AttributionKey, string>> = {}
  for (const key of allowedKeys) {
    const all = params.getAll(key)
    if (all.length > 1 || (all.length === 1 && !validValue(all[0]!))) return
    if (all.length === 1) values[key] = all[0]!
  }
  if (Object.keys(values).length === 0) return
  const record: AttributionRecord = { version: 1, capturedAt: (options.now ?? Date.now)(), values }
  const serialized = encode(record)
  if (new TextEncoder().encode(serialized).length > ATTRIBUTION_MAX_RECORD_BYTES) return
  try { storage.setItem(ATTRIBUTION_STORAGE_KEY, serialized) } catch { /* storage may be unavailable */ }
}

function readValid(storage: StorageLike, now: number): AttributionRecord | null {
  const raw = storage.getItem(ATTRIBUTION_STORAGE_KEY)
  if (!raw || new TextEncoder().encode(raw).length > ATTRIBUTION_MAX_RECORD_BYTES) return null
  try {
    const parsed = JSON.parse(raw) as AttributionRecord
    if (parsed.version !== 1 || !Number.isFinite(parsed.capturedAt) || now - parsed.capturedAt < 0 || now - parsed.capturedAt >= ATTRIBUTION_TTL_MS) return null
    const keys = Object.keys(parsed.values ?? {})
    if (keys.length === 0 || keys.some(key => !allowedKeys.includes(key as AttributionKey))) return null
    for (const key of keys) if (!validValue(parsed.values[key as AttributionKey] ?? "")) return null
    return parsed
  } catch { return null }
}

export async function bindStoredFleetAttribution(binder: AttributionBinder | undefined, options: Options = {}): Promise<void> {
  const storage = options.storage ?? storageDefault()
  if (!storage || !binder || !options.getAccessToken) return
  const record = readValid(storage, (options.now ?? Date.now)())
  if (!record) { storage.removeItem?.(ATTRIBUTION_STORAGE_KEY); return }
  const accessToken = await options.getAccessToken()
  if (!accessToken) return
  await binder.bind(record, accessToken)
  storage.removeItem?.(ATTRIBUTION_STORAGE_KEY)
}
