import assert from "node:assert/strict"
import test from "node:test"
import {
  ATTRIBUTION_MAX_RECORD_BYTES,
  ATTRIBUTION_MAX_VALUE_LENGTH,
  ATTRIBUTION_STORAGE_KEY,
  ATTRIBUTION_TTL_MS,
  bindStoredFleetAttribution,
  captureFleetAttribution,
  type AttributionBinder,
  type StorageLike,
} from "../src/auth/fleet-attribution.ts"

function memoryStorage(initial?: string): StorageLike & { values: Map<string, string> } {
  const values = new Map<string, string>()
  if (initial !== undefined) values.set(ATTRIBUTION_STORAGE_KEY, initial)
  return {
    values,
    getItem: key => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, value),
    removeItem: key => values.delete(key),
  }
}

test("first touch survives a Keycloak round trip and only binds validated public fields", async () => {
  const storage = memoryStorage()
  const firstTouch = 1_700_000_000_000

  captureFleetAttribution(
    "https://run.example/pools?campaign_id=launch-1&content_id=docs.v2&utm_source=search&utm_medium=cpc&utm_campaign=fall&unknown=drop-me",
    { storage, now: () => firstTouch },
  )
  captureFleetAttribution(
    "https://run.example/pools?code=oidc-code&state=opaque&campaign_id=overwrite",
    { storage, now: () => firstTouch + 1_000 },
  )

  const bound: unknown[] = []
  const binder: AttributionBinder = {
    bind: async record => {
      bound.push(record)
    },
  }
  await bindStoredFleetAttribution(binder, {
    storage,
    now: () => firstTouch + 2_000,
    getAccessToken: async () => "access-token",
  })

  assert.deepEqual(bound, [
    {
      version: 1,
      capturedAt: firstTouch,
      values: {
        campaign_id: "launch-1",
        content_id: "docs.v2",
        utm_source: "search",
        utm_medium: "cpc",
        utm_campaign: "fall",
      },
    },
  ])
})

test("invalid, malformed, empty, oversized, and expired records never reach a binder", async () => {
  const now = 1_700_000_000_000
  const invalidStoredRecords = [
    "not-json",
    JSON.stringify({}),
    JSON.stringify({ version: 1, capturedAt: now, values: {} }),
    JSON.stringify({ version: 1, capturedAt: now, values: { campaign_id: "has space" } }),
    JSON.stringify({ version: 1, capturedAt: now, values: { campaign_id: "x".repeat(ATTRIBUTION_MAX_VALUE_LENGTH + 1) } }),
    JSON.stringify({ version: 1, capturedAt: now - ATTRIBUTION_TTL_MS, values: { campaign_id: "expired" } }),
    "x".repeat(ATTRIBUTION_MAX_RECORD_BYTES + 1),
  ]

  for (const stored of invalidStoredRecords) {
    const storage = memoryStorage(stored)
    let calls = 0
    await bindStoredFleetAttribution({ bind: async () => { calls += 1 } }, { storage, now: () => now, getAccessToken: async () => "access-token" })
    assert.equal(calls, 0, stored.slice(0, 80))
    assert.equal(storage.getItem(ATTRIBUTION_STORAGE_KEY), null)
  }
})

test("capture rejects repeated, empty, invalid, and oversized values without truncation", () => {
  const invalidQueries = [
    "?campaign_id=",
    "?campaign_id=one&campaign_id=two",
    "?campaign_id=has%20space",
    `?campaign_id=${"x".repeat(ATTRIBUTION_MAX_VALUE_LENGTH + 1)}`,
  ]

  for (const query of invalidQueries) {
    const storage = memoryStorage()
    captureFleetAttribution(`https://run.example/${query}`, { storage, now: () => 10 })
    assert.equal(storage.values.size, 0, query)
  }
})

test("capture accepts channel-neutral organic and paid social values without provider logic", () => {
  for (const medium of ["organic-social", "paid-social"]) {
    const storage = memoryStorage()
    captureFleetAttribution(`https://run.example/?utm_source=x&utm_medium=${medium}`, {
      storage,
      now: () => 50,
    })
    const record = JSON.parse(storage.getItem(ATTRIBUTION_STORAGE_KEY) ?? "null")
    assert.deepEqual(record.values, { utm_source: "x", utm_medium: medium })
  }
})

test("TTL is valid immediately before the boundary and expired at the boundary", async () => {
  const capturedAt = 1_000
  const stored = JSON.stringify({
    version: 1,
    capturedAt,
    values: { campaign_id: "boundary" },
  })
  let calls = 0
  const binder: AttributionBinder = { bind: async () => { calls += 1 } }

  await bindStoredFleetAttribution(binder, {
    storage: memoryStorage(stored),
    now: () => capturedAt + ATTRIBUTION_TTL_MS - 1,
    getAccessToken: async () => "access-token",
  })
  await bindStoredFleetAttribution(binder, {
    storage: memoryStorage(stored),
    now: () => capturedAt + ATTRIBUTION_TTL_MS,
    getAccessToken: async () => "access-token",
  })

  assert.equal(calls, 1)
})

test("the binder is disabled by default", async () => {
  const storage = memoryStorage(JSON.stringify({
    version: 1,
    capturedAt: 100,
    values: { utm_source: "docs" },
  }))

  await bindStoredFleetAttribution(undefined, { storage, now: () => 101 })
  assert.notEqual(storage.getItem(ATTRIBUTION_STORAGE_KEY), null)
})
