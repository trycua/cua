import assert from "node:assert/strict"
import test from "node:test"
import {
  bindFleetAttribution,
  recordFleetLogin,
  recordFleetPaymentGate,
} from "../src/auth/analytics.ts"
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

test("records a privacy-safe payment gate for the authenticated session", async () => {
  const stored = new Map<string, string>()
  const calls: Array<{ input: string; init?: RequestInit }> = []
  await recordFleetPaymentGate("no_payment_method", "payment-session-1", {
    getToken: async () => "token-1",
    fetch: async (input, init) => {
      calls.push({ input, init })
      return new Response(null, { status: 204 })
    },
    storage: {
      getItem: key => stored.get(key) ?? null,
      setItem: (key, value) => stored.set(key, value),
    },
  })

  assert.equal(calls.length, 1)
  assert.equal(calls[0]?.input, "/api/analytics/payment-gate")
  assert.equal(calls[0]?.init?.method, "POST")
  assert.deepEqual(calls[0]?.init?.headers, {
    Authorization: "Bearer token-1",
    "Content-Type": "application/json",
  })
  assert.deepEqual(JSON.parse(String(calls[0]?.init?.body)), {
    reason: "no_payment_method",
  })
  assert.equal(stored.size, 1)
})

test("deduplicates concurrent and repeated payment gates within one session", async () => {
  const stored = new Map<string, string>()
  let calls = 0
  let releaseRequest: () => void = () => undefined
  const requestReleased = new Promise<void>(resolve => {
    releaseRequest = resolve
  })
  const dependencies = {
    getToken: async () => "token-1",
    fetch: async () => {
      calls += 1
      await requestReleased
      return new Response(null, { status: 204 })
    },
    storage: {
      getItem: (key: string) => stored.get(key) ?? null,
      setItem: (key: string, value: string) => stored.set(key, value),
    },
  }

  const first = recordFleetPaymentGate("no_payment_method", "payment-session-2", dependencies)
  const concurrent = recordFleetPaymentGate("no_payment_method", "payment-session-2", dependencies)
  releaseRequest()
  await Promise.all([first, concurrent])
  await recordFleetPaymentGate("no_payment_method", "payment-session-2", dependencies)

  assert.equal(calls, 1)
  assert.equal(stored.size, 1)
})

test("retries a payment gate after a failed request", async () => {
  const stored = new Map<string, string>()
  let calls = 0
  const dependencies = {
    getToken: async () => "token-1",
    fetch: async () => {
      calls += 1
      return new Response(null, { status: calls === 1 ? 503 : 204 })
    },
    storage: {
      getItem: (key: string) => stored.get(key) ?? null,
      setItem: (key: string, value: string) => stored.set(key, value),
    },
  }

  await assert.rejects(recordFleetPaymentGate("no_payment_method", "payment-session-3", dependencies))
  assert.equal(stored.size, 0)
  await recordFleetPaymentGate("no_payment_method", "payment-session-3", dependencies)

  assert.equal(calls, 2)
  assert.equal(stored.size, 1)
})

test("records separate payment gates for separate sessions and reasons", async () => {
  const stored = new Map<string, string>()
  let calls = 0
  const dependencies = {
    getToken: async () => "token-1",
    fetch: async () => {
      calls += 1
      return new Response(null, { status: 204 })
    },
    storage: {
      getItem: (key: string) => stored.get(key) ?? null,
      setItem: (key: string, value: string) => stored.set(key, value),
    },
  }

  await recordFleetPaymentGate("no_payment_method", "payment-session-4", dependencies)
  await recordFleetPaymentGate("no_payment_method", "payment-session-5", dependencies)
  await recordFleetPaymentGate("card_admission_required", "payment-session-4", dependencies)

  assert.equal(calls, 3)
  assert.equal(stored.size, 3)
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
