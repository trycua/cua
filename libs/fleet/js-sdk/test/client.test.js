import assert from "node:assert/strict"
import test from "node:test"

import { CyclopsClient } from "../dist/index.js"

test("generated endpoint uses its generated route and bearer authentication", async () => {
  const requests = []
  const fetch = async (input, init = {}) => {
    requests.push({ url: String(input), init })
    if (String(input).endsWith("/token")) {
      return Response.json({ access_token: "token-1", expires_in: 300 })
    }
    return Response.json({ keys: [] })
  }

  const client = new CyclopsClient({
    baseUrl: "https://run.example",
    tokenUrl: "https://auth.example/token",
    clientId: "key-example",
    clientSecret: "secret",
    fetch,
  })

  const response = await client.keys.keysList()

  assert.deepEqual(response.data, { keys: [] })
  assert.equal(requests[1].url, "https://run.example/api/keys")
  assert.equal(requests[1].init.method, "GET")
  assert.equal(requests[1].init.headers.Authorization, "Bearer token-1")
})

test("Node global fetch handles token and generated API requests", async () => {
  const originalFetch = globalThis.fetch
  const requests = []
  globalThis.fetch = async (input, init = {}) => {
    requests.push({ url: String(input), init })
    if (String(input).endsWith("/token")) {
      return Response.json({ access_token: "global-token", expires_in: 300 })
    }
    return Response.json({ keys: [] })
  }

  try {
    const client = new CyclopsClient({
      baseUrl: "https://run.example",
      tokenUrl: "https://auth.example/token",
      clientId: "key-example",
      clientSecret: "secret",
    })

    await client.keys.keysList()

    assert.equal(requests.length, 2)
    assert.equal(requests[1].init.headers.Authorization, "Bearer global-token")
  } finally {
    globalThis.fetch = originalFetch
  }
})

test("client fails clearly when fetch is unavailable", () => {
  const originalFetch = globalThis.fetch
  globalThis.fetch = undefined

  try {
    assert.throws(
      () =>
        new CyclopsClient({
          clientId: "key-example",
          clientSecret: "secret",
        }),
      /requires Node\.js 18\+ or an injected fetch implementation/,
    )
  } finally {
    globalThis.fetch = originalFetch
  }
})
