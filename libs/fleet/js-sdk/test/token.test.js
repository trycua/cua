import assert from "node:assert/strict"
import test from "node:test"

import { ClientCredentialsTokenProvider } from "../dist/index.js"

test("token provider exchanges credentials and reuses an unexpired token", async () => {
  const requests = []
  const provider = new ClientCredentialsTokenProvider({
    tokenUrl: "https://auth.example/token",
    clientId: "key-example",
    clientSecret: "secret",
    fetch: async (input, init = {}) => {
      requests.push({ url: String(input), init })
      return Response.json({ access_token: "token-1", expires_in: 300 })
    },
  })

  assert.equal(await provider.getToken(), "token-1")
  assert.equal(await provider.getToken(), "token-1")
  assert.equal(requests.length, 1)
  assert.equal(requests[0].url, "https://auth.example/token")
  assert.equal(requests[0].init.method, "POST")
  assert.equal(requests[0].init.body.get("grant_type"), "client_credentials")
  assert.equal(requests[0].init.body.get("client_id"), "key-example")
  assert.equal(requests[0].init.body.get("client_secret"), "secret")
})

test("token provider refreshes inside the configured expiry buffer", async () => {
  let exchanges = 0
  const provider = new ClientCredentialsTokenProvider({
    tokenUrl: "https://auth.example/token",
    clientId: "key-example",
    clientSecret: "secret",
    refreshBufferMs: 30_000,
    fetch: async () => {
      exchanges += 1
      return Response.json({ access_token: `token-${exchanges}`, expires_in: 1 })
    },
  })

  assert.equal(await provider.getToken(), "token-1")
  assert.equal(await provider.getToken(), "token-2")
})
