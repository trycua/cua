import assert from "node:assert/strict"
import test from "node:test"
import { bindFleetAttribution, recordFleetLogin } from "../src/auth/analytics.ts"
import { ATTRIBUTION_STORAGE_KEY } from "../src/auth/fleet-attribution.ts"

test("records one Fleet login per Keycloak browser session", async () => {
  const stored = new Map<string, string>()
  const calls: Array<{ input: string; init?: RequestInit }> = []
  const dependencies = {
    getToken: async () => "token-1",
    fetch: async (input: string, init?: RequestInit) => {
      calls.push({ input, init })
      return new Response(null, { status: 204 })
    },
    storage: {
      getItem: (key: string) => stored.get(key) ?? null,
      setItem: (key: string, value: string) => stored.set(key, value),
    },
  }

  await recordFleetLogin("session-1", dependencies)
  await recordFleetLogin("session-1", dependencies)

  assert.equal(calls.length, 1)
  assert.equal(calls[0]?.input, "/api/analytics/session")
  assert.equal(calls[0]?.init?.method, "POST")
  assert.equal((calls[0]?.init?.headers as Record<string, string>).Authorization, "Bearer token-1")
})

test("analytics failure does not mark the session as recorded", async () => {
  const stored = new Map<string, string>()
  const dependencies = {
    getToken: async () => "token-1",
    fetch: async () => new Response(null, { status: 503 }),
    storage: {
      getItem: (key: string) => stored.get(key) ?? null,
      setItem: (key: string, value: string) => stored.set(key, value),
    },
  }

  await assert.rejects(recordFleetLogin("session-2", dependencies))
  assert.equal(stored.size, 0)
})

test("binds validated first-touch attribution after authentication", async () => {
  const capturedAt = Date.now()
  const stored = new Map<string, string>([[ATTRIBUTION_STORAGE_KEY, JSON.stringify({
    version: 1,
    capturedAt,
    values: { utm_source: "x", utm_medium: "organic-social", utm_campaign: "openclaw-2-launch" },
  })]])
  const calls: Array<{ input: string; init?: RequestInit }> = []
  await bindFleetAttribution({
    getToken: async () => "token-1",
    fetch: async (input, init) => {
      calls.push({ input, init })
      return new Response(null, { status: 204 })
    },
    storage: {
      getItem: key => stored.get(key) ?? null,
      setItem: (key, value) => stored.set(key, value),
      removeItem: key => stored.delete(key),
    },
  })

  assert.equal(calls.length, 1)
  assert.equal(calls[0]?.input, "/api/analytics/attribution")
  assert.equal((calls[0]?.init?.headers as Record<string, string>).Authorization, "Bearer token-1")
  assert.deepEqual(JSON.parse(String(calls[0]?.init?.body)), {
    version: 1,
    capturedAt,
    values: { utm_source: "x", utm_medium: "organic-social", utm_campaign: "openclaw-2-launch" },
  })
  assert.equal(stored.has(ATTRIBUTION_STORAGE_KEY), false)
})

test("retains attribution when authenticated binding fails", async () => {
  const record = JSON.stringify({ version: 1, capturedAt: Date.now(), values: { utm_source: "x" } })
  const stored = new Map<string, string>([[ATTRIBUTION_STORAGE_KEY, record]])
  await assert.rejects(bindFleetAttribution({
    getToken: async () => "token-1",
    fetch: async () => new Response(null, { status: 503 }),
    storage: {
      getItem: key => stored.get(key) ?? null,
      setItem: (key, value) => stored.set(key, value),
      removeItem: key => stored.delete(key),
    },
  }))
  assert.equal(stored.get(ATTRIBUTION_STORAGE_KEY), record)
})
