// Mock API helpers for Playwright E2E tests.
//
// The cyclops-cs SPA authenticates via Keycloak PKCE (keycloak-js). In
// tests we intercept every outbound request so no real Keycloak or K8s
// cluster is needed.

import type { Page } from "@playwright/test"

// ---------------------------------------------------------------------------
// Keycloak auth mocking
// ---------------------------------------------------------------------------
// keycloak-js performs:
//   1. OIDC discovery: GET <kcUrl>/realms/<realm>/.well-known/openid-configuration
//   2. Token exchange (authorization_code or refresh): POST <tokenEndpoint>
//   3. Redirect to <authorizationEndpoint> when onLoad is "login-required"
//
// With onLoad: "login-required" the library checks for an existing session
// by looking at an iframe or checking URL params. If none, it redirects.
// Our strategy: intercept the well-known endpoint so the library gets
// valid-looking URLs, then intercept the auth redirect and short-circuit
// it by navigating back with a fake code. Finally, intercept the token
// endpoint to return a fake JWT.
//
// Simpler alternative chosen here: inject a script BEFORE the app bundle
// loads that replaces the Keycloak constructor with a stub. Vite serves
// the app as ES modules, and page.addInitScript runs before any module
// evaluation.

const FAKE_TOKEN =
  "eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9." +
  btoa(
    JSON.stringify({
      sub: "test-user-id",
      email: "test@example.com",
      preferred_username: "testuser",
      exp: Math.floor(Date.now() / 1000) + 3600,
      iat: Math.floor(Date.now() / 1000),
      iss: "https://auth.cua.ai/realms/cyclops-cs",
      aud: "cyclops-cs-spa",
    }),
  ) +
  ".fake-signature"

/**
 * Stub Keycloak auth so the AuthProvider renders children immediately.
 *
 * Must be called BEFORE page.goto() so the init script runs before the
 * app's ES modules are evaluated.
 *
 * Strategy: intercept the Vite-served keycloak-js module and replace it
 * with a stub class that resolves init() immediately as authenticated.
 * This avoids the real keycloak-js redirecting to the Keycloak login page.
 */
export async function mockAuth(page: Page): Promise<void> {
  // Intercept the Vite pre-bundled keycloak-js module. Vite serves
  // node_modules deps from /.vite/deps/ or /node_modules/.vite/deps/.
  // Replace the entire module with a stub Keycloak class.
  await page.route(/keycloak-js/, (route) => {
    route.fulfill({
      contentType: "application/javascript",
      body: `
        class Keycloak {
          constructor() {
            this.token = "${FAKE_TOKEN}";
            this.refreshToken = "fake-refresh";
            this.idToken = "${FAKE_TOKEN}";
            this.authenticated = true;
            this.tokenParsed = {
              sub: "test-user-id",
              email: "test@example.com",
              preferred_username: "testuser",
              azp: "cyclops-cs-spa",
              exp: Math.floor(Date.now() / 1000) + 3600,
            };
            this.realmAccess = { roles: [] };
            this.resourceAccess = {};
            this.subject = "test-user-id";
          }
          async init() { this.authenticated = true; return true; }
          async updateToken() { return true; }
          login() {}
          logout() { window.location.href = "/"; }
          register() {}
          accountManagement() {}
          createLoginUrl() { return "/"; }
          createLogoutUrl() { return "/"; }
          createRegisterUrl() { return "/"; }
          createAccountUrl() { return "/"; }
          isTokenExpired() { return false; }
          clearToken() {}
          hasRealmRole() { return false; }
          hasResourceRole() { return false; }
          loadUserProfile() { return Promise.resolve({ username: "testuser" }); }
          loadUserInfo() { return Promise.resolve({ sub: "test-user-id" }); }
        }
        Keycloak.default = Keycloak;
        export default Keycloak;
        export { Keycloak };
      `,
    })
  })

  // Mock the /api/config endpoint (feature flags)
  await page.route("**/api/config", (route) =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({ admin: false }),
    }),
  )

  // Intercept any stray Keycloak endpoint requests
  await page.route("**/realms/cyclops-cs/**", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: "{}" }),
  )
}

// ---------------------------------------------------------------------------
// Namespaces API mocking
// ---------------------------------------------------------------------------

export interface MockNamespace {
  name: string
  status: string
  createdAt: string
  labels: Record<string, string> | null
}

const DEFAULT_NAMESPACES: MockNamespace[] = [
  {
    name: "my-workspace",
    status: "Active",
    createdAt: "2026-01-15T10:00:00Z",
    labels: null,
  },
  {
    name: "staging",
    status: "Active",
    createdAt: "2026-02-01T12:00:00Z",
    labels: null,
  },
]

export async function mockNamespacesApi(
  page: Page,
  namespaces: MockNamespace[] = DEFAULT_NAMESPACES,
): Promise<void> {
  // The app calls /api/namespaces (GET, POST) and /api/namespaces/:name (DELETE)
  await page.route("**/api/namespaces", async (route) => {
    if (route.request().method() === "GET") {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify(namespaces),
      })
    } else if (route.request().method() === "POST") {
      const body = route.request().postDataJSON()
      const created: MockNamespace = {
        name: body.name,
        status: "Active",
        createdAt: new Date().toISOString(),
        labels: null,
      }
      namespaces.push(created)
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      })
    } else {
      await route.continue()
    }
  })

  await page.route("**/api/namespaces/*", async (route) => {
    if (route.request().method() === "DELETE") {
      const url = new URL(route.request().url())
      const name = url.pathname.split("/").pop()
      const idx = namespaces.findIndex((ns) => ns.name === name)
      if (idx >= 0) namespaces.splice(idx, 1)
      await route.fulfill({ status: 204, body: "" })
    } else {
      await route.continue()
    }
  })
}

// ---------------------------------------------------------------------------
// User API keys mocking
// ---------------------------------------------------------------------------

export interface MockUserApiKey {
  id: string
  client_id: string
  name: string
  scope: string[]
}

const DEFAULT_USER_KEYS: MockUserApiKey[] = [
  {
    id: "key-1",
    client_id: "sa-testuser-mykey",
    name: "my-key",
    scope: [],
  },
]

export async function mockUserKeysApi(
  page: Page,
  keys: MockUserApiKey[] = DEFAULT_USER_KEYS,
): Promise<void> {
  await page.route("**/api/user-keys", async (route) => {
    if (route.request().method() === "GET") {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ keys }),
      })
    } else if (route.request().method() === "POST") {
      const body = route.request().postDataJSON()
      const newKey: MockUserApiKey = {
        id: `key-${Date.now()}`,
        client_id: `sa-testuser-${body.name}`,
        name: body.name,
        scope: body.scope ?? [],
      }
      keys.push(newKey)
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify({
          client_id: newKey.client_id,
          client_secret: "fake-client-secret-shown-once",
          token_url:
            "https://auth.cua.ai/realms/cyclops-cs/protocol/openid-connect/token",
          name: newKey.name,
          scope: newKey.scope,
        }),
      })
    } else {
      await route.continue()
    }
  })

  await page.route("**/api/user-keys/*", async (route) => {
    if (route.request().method() === "DELETE") {
      const url = new URL(route.request().url())
      const id = url.pathname.split("/").pop()
      const idx = keys.findIndex((k) => k.id === id)
      if (idx >= 0) keys.splice(idx, 1)
      await route.fulfill({ status: 204, body: "" })
    } else {
      await route.continue()
    }
  })
}

// ---------------------------------------------------------------------------
// Pools API mocking (K8s CRD via kubectl-proxy sidecar)
// ---------------------------------------------------------------------------

export async function mockPoolsApi(page: Page): Promise<void> {
  // List pools: GET /api/k8s/apis/cua.ai/v1/osgymworkspacepools
  await page.route("**/api/k8s/apis/cua.ai/v1/osgymworkspacepools", async (route) => {
    if (route.request().method() === "GET") {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              metadata: { name: "demo-pool", namespace: "my-workspace" },
              spec: { replicas: 2 },
              status: { phase: "Ready", availableCount: 2 },
            },
          ],
        }),
      })
    } else {
      await route.continue()
    }
  })

  // Create pool: POST /api/k8s/apis/cua.ai/v1/namespaces/*/osgymworkspacepools
  await page.route(
    "**/api/k8s/apis/cua.ai/v1/namespaces/*/osgymworkspacepools",
    async (route) => {
      if (route.request().method() === "POST") {
        const body = route.request().postDataJSON()
        await route.fulfill({
          status: 201,
          contentType: "application/json",
          body: JSON.stringify({
            metadata: {
              name: body.metadata.name,
              namespace: route.request().url().split("/namespaces/")[1].split("/")[0],
            },
            spec: body.spec,
            status: { phase: "Provisioning" },
          }),
        })
      } else {
        await route.continue()
      }
    },
  )
}

// ---------------------------------------------------------------------------
// Claims API mocking (OSGymSandboxClaim CRD)
// ---------------------------------------------------------------------------

export async function mockClaimsApi(page: Page): Promise<void> {
  await page.route(
    "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/*/osgymsandboxclaims",
    async (route) => {
      if (route.request().method() === "GET") {
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify({
            items: [
              {
                metadata: {
                  name: "claim-abc123",
                  namespace: "pool-demo-pool",
                  creationTimestamp: "2026-05-28T10:00:00Z",
                },
                spec: {
                  sandboxTemplateRef: { name: "demo-pool-template" },
                  warmpool: "default",
                },
                status: {
                  phase: "Bound",
                  sandbox: {
                    name: "vm-xyz789",
                    service: "vm-xyz789-svc",
                  },
                },
              },
            ],
          }),
        })
      } else if (route.request().method() === "POST") {
        const body = route.request().postDataJSON()
        await route.fulfill({
          status: 201,
          contentType: "application/json",
          body: JSON.stringify({
            metadata: {
              name: body.metadata.name,
              namespace: "pool-demo-pool",
              creationTimestamp: new Date().toISOString(),
            },
            spec: body.spec,
            status: { phase: "Pending" },
          }),
        })
      } else {
        await route.continue()
      }
    },
  )

  // DELETE for individual claims
  await page.route(
    "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/*/osgymsandboxclaims/*",
    async (route) => {
      if (route.request().method() === "DELETE") {
        await route.fulfill({ status: 200, body: "{}" })
      } else {
        await route.continue()
      }
    },
  )
}

// ---------------------------------------------------------------------------
// Pool templates API mocking (/api/pool-templates — Redis-backed)
// ---------------------------------------------------------------------------

export interface MockPoolTemplate {
  user: string
  name: string
  createdAt: string
  config: Record<string, unknown>
}

const DEFAULT_POOL_TEMPLATES: MockPoolTemplate[] = [
  {
    user: "test-user-id",
    name: "gpu-large",
    createdAt: "2026-05-01T10:00:00Z",
    config: {
      cpu: 8,
      ram: "16Gi",
      ociImage: "example/osgym-workspace:latest",
      replicas: 2,
    },
  },
]

export async function mockPoolTemplatesApi(
  page: Page,
  templates: MockPoolTemplate[] = DEFAULT_POOL_TEMPLATES,
): Promise<void> {
  // GET list + POST create
  await page.route("**/api/pool-templates", async (route) => {
    if (route.request().method() === "GET") {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify(templates),
      })
    } else if (route.request().method() === "POST") {
      const body = route.request().postDataJSON()
      const created: MockPoolTemplate = {
        user: "test-user-id",
        name: body.name,
        createdAt: new Date().toISOString(),
        config: body.config,
      }
      const idx = templates.findIndex((t) => t.name === body.name)
      if (idx >= 0) templates[idx] = created
      else templates.push(created)
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      })
    } else {
      await route.continue()
    }
  })

  // DELETE /api/pool-templates/:name
  await page.route("**/api/pool-templates/*", async (route) => {
    if (route.request().method() === "DELETE") {
      const url = new URL(route.request().url())
      const name = decodeURIComponent(url.pathname.split("/").pop() ?? "")
      const idx = templates.findIndex((t) => t.name === name)
      if (idx < 0) {
        await route.fulfill({
          status: 404,
          contentType: "application/json",
          body: JSON.stringify({ error: "template not found" }),
        })
        return
      }
      templates.splice(idx, 1)
      await route.fulfill({ status: 204, body: "" })
    } else {
      await route.continue()
    }
  })
}
