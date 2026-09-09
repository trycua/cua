import assert from "node:assert/strict"
import test from "node:test"
import {
  appLoginRedirectUri,
  appLogoutRedirectUri,
} from "../src/auth/redirects.ts"

test("login redirect preserves the current Run route and ordinary parameters", () => {
  assert.equal(
    appLoginRedirectUri("https://run.cua.ai/pools/demo?tab=activity#logs"),
    "https://run.cua.ai/pools/demo?tab=activity#logs",
  )
})

test("login redirect strips a successful OIDC query callback", () => {
  assert.equal(
    appLoginRedirectUri(
      "https://run.cua.ai/settings?tab=billing&code=one-time&state=opaque&session_state=opaque&iss=https%3A%2F%2Fauth.cua.ai%2Frealms%2Fcyclops-cs",
    ),
    "https://run.cua.ai/settings?tab=billing",
  )
})

test("login redirect strips a rejected OIDC fragment callback", () => {
  assert.equal(
    appLoginRedirectUri(
      "https://run.cua.ai/pools#state=opaque&error=login_required&error_description=expired",
    ),
    "https://run.cua.ai/pools",
  )
})

test("login redirect does not remove unrelated application errors", () => {
  assert.equal(
    appLoginRedirectUri("https://run.cua.ai/pools?error=capacity"),
    "https://run.cua.ai/pools?error=capacity",
  )
})

test("logout redirect is pinned to the same-origin Run root", () => {
  assert.equal(
    appLogoutRedirectUri("https://run.cua.ai/settings?tab=billing"),
    "https://run.cua.ai/",
  )
  assert.equal(
    appLogoutRedirectUri("http://localhost:9090/pools/demo"),
    "http://localhost:9090/",
  )
})
