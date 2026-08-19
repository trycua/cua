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
export interface MockFeatureFlags {
  admin?: boolean
  billing?: boolean
  chat?: boolean
  usage?: boolean
}

export interface MockAuthOptions {
  holdConfig?: boolean
  holdAuth?: boolean
  initRejects?: boolean
  refreshFails?: boolean
}

export interface MockAuthControl {
  releaseConfig(): void
  releaseAuth(): Promise<void>
}

export async function mockAuth(
  page: Page,
  flags: MockFeatureFlags = {},
  options: MockAuthOptions = {},
): Promise<MockAuthControl> {
  const config = deferred()
  if (!options.holdConfig) config.resolve()
  const holdAuth = options.holdAuth ?? false
  const initRejects = options.initRejects ?? false
  const refreshFails = options.refreshFails ?? false
  // Intercept the Vite pre-bundled keycloak-js module. Vite serves
  // node_modules deps from /.vite/deps/ or /node_modules/.vite/deps/.
  // Replace the entire module with a stub Keycloak class.
  await page.route(/keycloak-js/, (route) => {
    route.fulfill({
      contentType: "application/javascript",
      body: `
        const authReady = new Promise(resolve => {
          window.__MOCK_KEYCLOAK__ = {
            initOptions: null,
            loginOptions: null,
            logoutOptions: null,
            updateTokenMinValidity: [],
            clearTokenCalls: 0,
            releaseAuth: resolve,
          };
        });
        ${holdAuth ? "" : "window.__MOCK_KEYCLOAK__.releaseAuth();"}

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
          async init(options) {
            window.__MOCK_KEYCLOAK__.initOptions = options;
            await authReady;
            ${initRejects ? 'throw new Error("rejected callback");' : ""}
            this.authenticated = true;
            return true;
          }
          async updateToken(minValidity) {
            window.__MOCK_KEYCLOAK__.updateTokenMinValidity.push(minValidity);
            ${refreshFails ? 'throw new Error("refresh failed");' : ""}
            return true;
          }
          login(options) {
            window.__MOCK_KEYCLOAK__.loginOptions = options;
            return Promise.resolve();
          }
          logout(options) {
            window.__MOCK_KEYCLOAK__.logoutOptions = options;
            return Promise.resolve();
          }
          register() {}
          accountManagement() {}
          createLoginUrl() { return "/"; }
          createLogoutUrl() { return "/"; }
          createRegisterUrl() { return "/"; }
          createAccountUrl() { return "/"; }
          isTokenExpired() { return false; }
          clearToken() {
            window.__MOCK_KEYCLOAK__.clearTokenCalls += 1;
            this.authenticated = false;
          }
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

  await page.route("**/api/config", async route => {
    await config.promise
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        admin: flags.admin ?? false,
        billing: flags.billing ?? false,
        chat: flags.chat ?? false,
        usage: flags.usage ?? false,
      }),
    })
  })

  // Intercept any stray Keycloak endpoint requests
  await page.route("**/realms/cyclops-cs/**", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: "{}" }),
  )

  return {
    releaseConfig: config.resolve,
    releaseAuth: () =>
      page.evaluate(() => {
        const control = (window as unknown as {
          __MOCK_KEYCLOAK__: { releaseAuth(): void }
        }).__MOCK_KEYCLOAK__
        control.releaseAuth()
      }),
  }
}

// ---------------------------------------------------------------------------
// Feature flags / per-user config mocking
// ---------------------------------------------------------------------------

export async function mockConfigApi(
  page: Page,
  config: { admin: boolean; billing?: boolean },
): Promise<void> {
  await page.route("**/api/config", route =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({ billing: false, ...config }),
    }),
  )
}

// ---------------------------------------------------------------------------
// Admin feature flags API mocking
// ---------------------------------------------------------------------------

export type MockFeatureFlagValueType = "boolean" | "number" | "string" | "json"
export type MockFeatureFlagOwnership = "terraform" | "ad_hoc" | "external"

export interface MockAdminFeatureFlag {
  key: string
  path: string
  value_type: MockFeatureFlagValueType
  value: unknown
  raw_value: string
  ownership: MockFeatureFlagOwnership
  managed_by?: string
  deletable: boolean
  version: number
  modified_at: string
  description?: string
  tags: Record<string, string>
}

export interface MockFeatureFlagRequest {
  method: string
  key?: string
  body?: Record<string, unknown>
}

export const DEFAULT_ADMIN_FEATURE_FLAGS: MockAdminFeatureFlag[] = [
  {
    key: "admin-subs",
    path: "/feature-flags/cyclops-cs/admin-subs",
    value_type: "json",
    value: ["test-user-id", "other-admin"],
    raw_value: '["test-user-id","other-admin"]',
    ownership: "terraform",
    managed_by: "terraform",
    deletable: false,
    version: 7,
    modified_at: "2026-08-12T10:15:00Z",
    description: "Cyclops administrators",
    tags: { ManagedBy: "Terraform" },
  },
  {
    key: "new-checkout",
    path: "/feature-flags/cyclops-cs/new-checkout",
    value_type: "boolean",
    value: true,
    raw_value: "true",
    ownership: "ad_hoc",
    managed_by: "cyclops-cs",
    deletable: true,
    version: 3,
    modified_at: "2026-08-13T08:30:00Z",
    description: "Enable the checkout experiment",
    tags: { ManagedBy: "cyclops-cs" },
  },
  {
    key: "legacy-threshold",
    path: "/feature-flags/cyclops-cs/legacy-threshold",
    value_type: "number",
    value: 12.5,
    raw_value: "12.5",
    ownership: "external",
    managed_by: "migration-tool",
    deletable: false,
    version: 11,
    modified_at: "2026-08-11T17:45:00Z",
    tags: {},
  },
]

function rawFeatureFlagValue(value: unknown, valueType: MockFeatureFlagValueType): string {
  if (valueType === "string") return String(value)
  return JSON.stringify(value)
}

export async function mockAdminFeatureFlagsApi(
  page: Page,
  initialFlags: MockAdminFeatureFlag[] = DEFAULT_ADMIN_FEATURE_FLAGS,
): Promise<{ flags: MockAdminFeatureFlag[]; requests: MockFeatureFlagRequest[] }> {
  const flags = initialFlags.map(flag => ({ ...flag, tags: { ...flag.tags } }))
  const requests: MockFeatureFlagRequest[] = []

  await page.route(/\/api\/admin\/feature-flags(?:\/[^/?]+)?(?:\?.*)?$/, async route => {
    const request = route.request()
    const url = new URL(request.url())
    const pathParts = url.pathname.split("/").filter(Boolean)
    const encodedKey = pathParts.length > 3 ? pathParts.at(-1) : undefined
    const key = encodedKey ? decodeURIComponent(encodedKey) : undefined
    const body = request.postData() ? request.postDataJSON() as Record<string, unknown> : undefined
    requests.push({ method: request.method(), key, body })

    if (request.method() === "GET" && !key) {
      await route.fulfill({ contentType: "application/json", body: JSON.stringify(flags) })
      return
    }

    if (request.method() === "POST" && !key && body) {
      const valueType = body.value_type as MockFeatureFlagValueType
      const created: MockAdminFeatureFlag = {
        key: String(body.key),
        path: `/feature-flags/cyclops-cs/${String(body.key)}`,
        value_type: valueType,
        value: body.value,
        raw_value: rawFeatureFlagValue(body.value, valueType),
        ownership: "ad_hoc",
        managed_by: "cyclops-cs",
        deletable: true,
        version: 1,
        modified_at: "2026-08-13T12:00:00Z",
        description: body.description ? String(body.description) : undefined,
        tags: { ManagedBy: "cyclops-cs" },
      }
      flags.push(created)
      await route.fulfill({ status: 201, contentType: "application/json", body: JSON.stringify(created) })
      return
    }

    const index = flags.findIndex(flag => flag.key === key)
    if (index < 0) {
      await route.fulfill({ status: 404, contentType: "application/json", body: JSON.stringify({ code: "flag_not_found", message: "feature flag not found" }) })
      return
    }

    if (request.method() === "PUT" && body) {
      const valueType = body.value_type as MockFeatureFlagValueType
      flags[index] = {
        ...flags[index],
        value_type: valueType,
        value: body.value,
        raw_value: rawFeatureFlagValue(body.value, valueType),
        version: flags[index].version + 1,
        modified_at: "2026-08-13T12:05:00Z",
      }
      await route.fulfill({ contentType: "application/json", body: JSON.stringify(flags[index]) })
      return
    }

    if (request.method() === "DELETE") {
      flags.splice(index, 1)
      await route.fulfill({ status: 204, body: "" })
      return
    }

    await route.continue()
  })

  return { flags, requests }
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
    name: "demo-pool",
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
// GitHub trust policies mocking
// ---------------------------------------------------------------------------

export interface MockGitHubTrustPolicy {
  id: string
  owner_sub: string
  name: string
  repository: string
  allowed_namespaces: string[]
  enabled: boolean
  created_at: string
  updated_at: string
}

const DEFAULT_GITHUB_TRUST_POLICIES: MockGitHubTrustPolicy[] = [
  {
    id: "gh-policy-1",
    owner_sub: "test-user-id",
    name: "cloud-ci",
    repository: "trycua/cloud",
    allowed_namespaces: ["my-workspace", "preview-a"],
    enabled: true,
    created_at: "2026-06-26T10:00:00Z",
    updated_at: "2026-06-26T10:30:00Z",
  },
]

export async function mockGitHubTrustPoliciesApi(
  page: Page,
  policies: MockGitHubTrustPolicy[] = DEFAULT_GITHUB_TRUST_POLICIES,
): Promise<void> {
  await page.route("**/api/github-trust-policies", async (route) => {
    if (route.request().method() === "GET") {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          policies,
          oidc: {
            issuer: "https://token.actions.githubusercontent.com",
            audience: "fleets",
          },
        }),
      })
      return
    }
    if (route.request().method() === "POST") {
      const body = route.request().postDataJSON()
      const created: MockGitHubTrustPolicy = {
        id: `gh-policy-${Date.now()}`,
        owner_sub: "test-user-id",
        name: body.name,
        repository: body.repository,
        allowed_namespaces: body.allowed_namespaces ?? [],
        enabled: body.enabled ?? true,
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
      }
      policies.unshift(created)
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      })
      return
    }
    await route.continue()
  })

  await page.route("**/api/github-trust-policies/*", async (route) => {
    const url = new URL(route.request().url())
    const id = url.pathname.split("/").pop()
    const policy = policies.find(item => item.id === id)
    if (route.request().method() === "PATCH") {
      if (!policy) {
        await route.fulfill({ status: 404, body: JSON.stringify({ error: "not found" }) })
        return
      }
      const body = route.request().postDataJSON()
      Object.assign(policy, body, { updated_at: new Date().toISOString() })
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify(policy),
      })
      return
    }
    if (route.request().method() === "DELETE") {
      const idx = policies.findIndex(item => item.id === id)
      if (idx >= 0) policies.splice(idx, 1)
      await route.fulfill({ status: 204, body: "" })
      return
    }
    await route.continue()
  })
}

// ---------------------------------------------------------------------------
// Pools API mocking (K8s CRD via kubectl-proxy sidecar)
// ---------------------------------------------------------------------------

export async function mockPoolsApi(page: Page): Promise<void> {
  const pool = {
    apiVersion: "osgym.cua.ai/v1alpha1",
    kind: "OSGymSandboxWarmPool",
    metadata: { name: "demo-pool", namespace: "demo-pool" },
    spec: {
      replicas: 2,
      sandboxTemplateRef: { name: "demo-pool-template" },
    },
    status: { replicas: 2, readyReplicas: 2 },
  }
  const template = {
    apiVersion: "osgym.cua.ai/v1alpha1",
    kind: "OSGymSandboxTemplate",
    metadata: { name: "demo-pool-template", namespace: "demo-pool" },
    spec: {
      vmTemplate: {
        containerDiskImage: "test-image:latest",
        cpuCores: 4,
        memory: "4Gi",
        services: [],
      },
    },
  }

  await page.route(
    "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/*/osgymsandboxwarmpools",
    async (route) => {
      const namespace = route.request().url().split("/namespaces/")[1].split("/")[0]
      if (route.request().method() === "GET") {
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify({ items: namespace === "demo-pool" ? [pool] : [] }),
        })
      } else if (route.request().method() === "POST") {
        const body = route.request().postDataJSON()
        await route.fulfill({
          status: 201,
          contentType: "application/json",
          body: JSON.stringify({ ...body, status: { replicas: body.spec.replicas } }),
        })
      } else {
        await route.continue()
      }
    },
  )

  await page.route(
    "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/demo-pool/osgymsandboxwarmpools/demo-pool",
    async (route) => {
      if (route.request().method() === "GET") {
        await route.fulfill({ contentType: "application/json", body: JSON.stringify(pool) })
      } else if (route.request().method() === "DELETE") {
        await route.fulfill({ status: 204, body: "" })
      } else {
        await route.continue()
      }
    },
  )

  await page.route(
    "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/*/osgymsandboxtemplates",
    async (route) => {
      if (route.request().method() === "POST") {
        await route.fulfill({
          status: 201,
          contentType: "application/json",
          body: route.request().postData() ?? "{}",
        })
      } else {
        await route.continue()
      }
    },
  )

  await page.route(
    "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/demo-pool/osgymsandboxtemplates/demo-pool-template",
    async (route) => {
      await route.fulfill({ contentType: "application/json", body: JSON.stringify(template) })
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
                apiVersion: "osgym.cua.ai/v1alpha1",
                kind: "OSGymSandboxClaim",
                metadata: {
                  name: "claim-abc123",
                  namespace: "demo-pool",
                  creationTimestamp: "2026-05-28T10:00:00Z",
                },
                spec: {
                  sandboxTemplateRef: { name: "demo-pool-template" },
                  warmpool: "demo-pool",
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
            apiVersion: "osgym.cua.ai/v1alpha1",
            kind: "OSGymSandboxClaim",
            metadata: {
              name: body.metadata.name,
              namespace: "demo-pool",
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
// Chat API mocking
// ---------------------------------------------------------------------------

export interface MockChatMessage {
  id: string
  role: "user" | "assistant" | "tool"
  content: string
  created_at: string
  tool_call_id?: string
  tool_calls?: Array<{
    id: string
    type: "function"
    function: { name: string; arguments: string }
  }>
}

export interface MockChatConversation {
  id: string
  title: string
  created_at: string
  updated_at: string
  messages: MockChatMessage[]
}

export interface MockChatApiOptions {
  conversations?: MockChatConversation[]
  holdList?: boolean
  holdConversation?: boolean
  turns?: MockChatTurn[]
  refreshError?: { afterTurnRequests: number; message: string }
  holdRefresh?: boolean
  createError?: { message: string }
}

export interface MockChatApiControl {
  authorizationHeaders: string[]
  createRequests: number
  turnRequests: Array<{ conversationId: string; messages: Array<Record<string, unknown>> }>
  releaseList(): void
  releaseConversation(): void
  releaseRefresh(): void
  releaseTurn(): void
}

export interface MockChatTurn {
  events?: Array<Record<string, unknown>>
  error?: { status: number; message: string }
  hold?: boolean
}

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve = () => {}
  const promise = new Promise<void>(complete => {
    resolve = complete
  })
  return { promise, resolve }
}

function defaultChatConversations(): MockChatConversation[] {
  const now = new Date().toISOString()
  return [
    {
      id: "conversation-1",
      title: "Example browser task",
      created_at: now,
      updated_at: now,
      messages: [
        {
          id: "message-1",
          role: "user",
          content: "Open the example site.",
          created_at: now,
        },
        {
          id: "message-2",
          role: "assistant",
          content: "The example site is ready.",
          created_at: now,
        },
      ],
    },
  ]
}

export async function mockChatApi(
  page: Page,
  options: MockChatApiOptions = {},
): Promise<MockChatApiControl> {
  const conversations = options.conversations ?? defaultChatConversations()
  const authorizationHeaders: string[] = []
  const createRequests = { count: 0 }
  const turnRequests: Array<{ conversationId: string; messages: Array<Record<string, unknown>> }> = []
  const list = deferred()
  const conversation = deferred()
  const refresh = deferred()
  const turnGate = deferred()
  const turns = [...(options.turns ?? [])]
  let refreshFailuresRemaining = options.refreshError ? 1 : 0
  let createFailuresRemaining = options.createError ? 1 : 0

  if (!options.holdList) list.resolve()
  if (!options.holdConversation) conversation.resolve()
  if (!options.holdRefresh) refresh.resolve()
  if (!turns.some(item => item.hold)) turnGate.resolve()

  await page.route("**/api/chat/conversations", async route => {
    if (route.request().method() === "GET") {
      authorizationHeaders.push(route.request().headers().authorization ?? "")
      if (options.refreshError && turnRequests.length >= options.refreshError.afterTurnRequests && refreshFailuresRemaining > 0) {
        refreshFailuresRemaining -= 1
        await route.fulfill({ status: 500, contentType: "application/json", body: JSON.stringify({ error: options.refreshError.message }) })
        return
      }
      await list.promise
      if (options.holdRefresh && turnRequests.length > 0) await refresh.promise
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify(
          conversations.map(({ messages: _messages, ...summary }) => summary),
        ),
      })
      return
    }

    if (route.request().method() === "POST") {
      createRequests.count += 1
      if (createFailuresRemaining > 0) {
        createFailuresRemaining -= 1
        await route.fulfill({ status: 500, contentType: "application/json", body: JSON.stringify({ error: options.createError!.message }) })
        return
      }
      const now = new Date().toISOString()
      const created: MockChatConversation = {
        id: `conversation-${conversations.length + 1}`,
        title: "New conversation",
        created_at: now,
        updated_at: now,
        messages: [],
      }
      conversations.unshift(created)
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      })
      return
    }

    await route.continue()
  })

  await page.route("**/api/chat/conversations/*", async route => {
    if (route.request().method() !== "GET") {
      await route.continue()
      return
    }

    if (options.refreshError && turnRequests.length >= options.refreshError.afterTurnRequests && refreshFailuresRemaining > 0) {
      refreshFailuresRemaining -= 1
      await route.fulfill({ status: 500, contentType: "application/json", body: JSON.stringify({ error: options.refreshError.message }) })
      return
    }
    await conversation.promise
    if (options.holdRefresh && turnRequests.length > 0) await refresh.promise
    const id = new URL(route.request().url()).pathname.split("/").pop()
    const selected = conversations.find(item => item.id === id)
    if (!selected) {
      await route.fulfill({
        status: 404,
        contentType: "application/json",
        body: JSON.stringify({ error: "conversation not found" }),
      })
      return
    }
    await route.fulfill({ contentType: "application/json", body: JSON.stringify(selected) })
  })

  await page.route("**/api/chat/conversations/*/turns", async route => {
    if (route.request().method() !== "POST") {
      await route.continue()
      return
    }

    const url = new URL(route.request().url())
    const conversationId = url.pathname.split("/").slice(-2, -1)[0]
    const body = route.request().postDataJSON() as { messages?: Array<Record<string, unknown>> }
    turnRequests.push({ conversationId, messages: body.messages ?? [] })
    const now = new Date().toISOString()
    const selected = conversations.find(item => item.id === conversationId)
    if (selected) {
      for (const message of body.messages ?? []) {
        selected.messages.push({ id: `message-${selected.messages.length + 1}`, role: message.role as MockChatMessage["role"], content: String(message.content ?? ""), created_at: now, ...(typeof message.tool_call_id === "string" ? { tool_call_id: message.tool_call_id } : {}) })
      }
      selected.updated_at = now
    }
    const turn = turns.shift()
    if (!turn) {
      await route.fulfill({ status: 500, contentType: "application/json", body: JSON.stringify({ error: "No mock turn" }) })
      return
    }
    if (turn.hold) await turnGate.promise
    if (turn.error) {
      await route.fulfill({
        status: turn.error.status,
        contentType: "application/json",
        body: JSON.stringify({ error: turn.error.message }),
      })
      return
    }
    if (selected) {
      const final = [...(turn.events ?? [])].reverse().find(event => event.type === "assistant")?.message as Record<string, unknown> | undefined
      if (final) selected.messages.push({ id: `message-${selected.messages.length + 1}`, role: "assistant", content: String(final.content ?? ""), created_at: now, ...(Array.isArray(final.tool_calls) ? { tool_calls: final.tool_calls as MockChatMessage["tool_calls"] } : {}) })
    }
    await route.fulfill({
      contentType: "application/x-ndjson",
      body: (turn.events ?? []).map(event => JSON.stringify(event) + "\n").join(""),
    })
  })

  return {
    authorizationHeaders,
    get createRequests() { return createRequests.count },
    turnRequests,
    releaseList: list.resolve,
    releaseConversation: conversation.resolve,
    releaseRefresh: refresh.resolve,
    releaseTurn: turnGate.resolve,
  }
}
