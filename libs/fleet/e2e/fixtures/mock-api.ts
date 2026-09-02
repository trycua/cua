// Mock API helpers for Playwright E2E tests.
//
// The cyclops-cs SPA authenticates via Keycloak PKCE (keycloak-js). In
// tests we intercept every outbound request so no real Keycloak or K8s
// cluster is needed.

import type { Page } from "@playwright/test";

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
  ".fake-signature";

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
  admin?: boolean;
  billing?: boolean;
  chat?: boolean;
  usage?: boolean;
  usagePricing?: { vcpuHourUSD: number; memoryGiBHourUSD: number };
}

export interface MockAuthOptions {
  holdConfig?: boolean;
  holdAuth?: boolean;
  initRejects?: boolean;
  refreshFails?: boolean;
}

export interface MockAuthControl {
  releaseConfig(): void;
  releaseAuth(): Promise<void>;
}

export async function mockAuth(
  page: Page,
  flags: MockFeatureFlags = {},
  options: MockAuthOptions = {},
): Promise<MockAuthControl> {
  const config = deferred();
  if (!options.holdConfig) config.resolve();
  const holdAuth = options.holdAuth ?? false;
  const initRejects = options.initRejects ?? false;
  const refreshFails = options.refreshFails ?? false;
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
    });
  });

  await page.route("**/api/config", async (route) => {
    await config.promise;
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({
        admin: flags.admin ?? false,
        billing: flags.billing ?? false,
        chat: flags.chat ?? false,
        usage: flags.usage ?? false,
        usage_pricing: {
          vcpu_hour_usd: flags.usagePricing?.vcpuHourUSD ?? 0.044625,
          memory_gib_hour_usd: flags.usagePricing?.memoryGiBHourUSD ?? 0.0223125,
        },
      }),
    });
  });

  // Intercept any stray Keycloak endpoint requests
  await page.route("**/realms/cyclops-cs/**", (route) =>
    route.fulfill({ status: 200, contentType: "application/json", body: "{}" }),
  );

  return {
    releaseConfig: config.resolve,
    releaseAuth: () =>
      page.evaluate(() => {
        const control = (
          window as unknown as {
            __MOCK_KEYCLOAK__: { releaseAuth(): void };
          }
        ).__MOCK_KEYCLOAK__;
        control.releaseAuth();
      }),
  };
}

// ---------------------------------------------------------------------------
// Feature flags / per-user config mocking
// ---------------------------------------------------------------------------

export async function mockConfigApi(
  page: Page,
  config: { admin: boolean; billing?: boolean },
): Promise<void> {
  await page.route("**/api/config", (route) =>
    route.fulfill({
      contentType: "application/json",
      body: JSON.stringify({ billing: false, ...config }),
    }),
  );
}

// ---------------------------------------------------------------------------
// Admin feature flags API mocking
// ---------------------------------------------------------------------------

export type MockFeatureFlagValueType = "boolean" | "number" | "string" | "json";
export type MockFeatureFlagOwnership = "terraform" | "ad_hoc" | "external";

export interface MockAdminFeatureFlag {
  key: string;
  path: string;
  value_type: MockFeatureFlagValueType;
  value: unknown;
  raw_value: string;
  ownership: MockFeatureFlagOwnership;
  managed_by?: string;
  deletable: boolean;
  version: number;
  modified_at: string;
  description?: string;
  tags: Record<string, string>;
}

export interface MockFeatureFlagRequest {
  method: string;
  key?: string;
  body?: Record<string, unknown>;
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
];

function rawFeatureFlagValue(
  value: unknown,
  valueType: MockFeatureFlagValueType,
): string {
  if (valueType === "string") return String(value);
  return JSON.stringify(value);
}

export async function mockAdminFeatureFlagsApi(
  page: Page,
  initialFlags: MockAdminFeatureFlag[] = DEFAULT_ADMIN_FEATURE_FLAGS,
): Promise<{
  flags: MockAdminFeatureFlag[];
  requests: MockFeatureFlagRequest[];
}> {
  const flags = initialFlags.map((flag) => ({
    ...flag,
    tags: { ...flag.tags },
  }));
  const requests: MockFeatureFlagRequest[] = [];

  await page.route(
    /\/api\/admin\/feature-flags(?:\/[^/?]+)?(?:\?.*)?$/,
    async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      const pathParts = url.pathname.split("/").filter(Boolean);
      const encodedKey = pathParts.length > 3 ? pathParts.at(-1) : undefined;
      const key = encodedKey ? decodeURIComponent(encodedKey) : undefined;
      const body = request.postData()
        ? (request.postDataJSON() as Record<string, unknown>)
        : undefined;
      requests.push({ method: request.method(), key, body });

      if (request.method() === "GET" && !key) {
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify(flags),
        });
        return;
      }

      if (request.method() === "POST" && !key && body) {
        const valueType = body.value_type as MockFeatureFlagValueType;
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
        };
        flags.push(created);
        await route.fulfill({
          status: 201,
          contentType: "application/json",
          body: JSON.stringify(created),
        });
        return;
      }

      const index = flags.findIndex((flag) => flag.key === key);
      if (index < 0) {
        await route.fulfill({
          status: 404,
          contentType: "application/json",
          body: JSON.stringify({
            code: "flag_not_found",
            message: "feature flag not found",
          }),
        });
        return;
      }

      if (request.method() === "PUT" && body) {
        const valueType = body.value_type as MockFeatureFlagValueType;
        flags[index] = {
          ...flags[index],
          value_type: valueType,
          value: body.value,
          raw_value: rawFeatureFlagValue(body.value, valueType),
          version: flags[index].version + 1,
          modified_at: "2026-08-13T12:05:00Z",
        };
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify(flags[index]),
        });
        return;
      }

      if (request.method() === "DELETE") {
        flags.splice(index, 1);
        await route.fulfill({ status: 204, body: "" });
        return;
      }

      await route.continue();
    },
  );

  return { flags, requests };
}

// ---------------------------------------------------------------------------
// Namespaces API mocking
// ---------------------------------------------------------------------------

export interface MockNamespace {
  name: string;
  status: string;
  createdAt: string;
  labels: Record<string, string> | null;
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
];

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
      });
    } else if (route.request().method() === "POST") {
      const body = route.request().postDataJSON();
      const created: MockNamespace = {
        name: body.name,
        status: "Active",
        createdAt: new Date().toISOString(),
        labels: null,
      };
      namespaces.push(created);
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      });
    } else {
      await route.continue();
    }
  });

  await page.route("**/api/namespaces/*", async (route) => {
    if (route.request().method() === "DELETE") {
      const url = new URL(route.request().url());
      const name = url.pathname.split("/").pop();
      const idx = namespaces.findIndex((ns) => ns.name === name);
      if (idx >= 0) namespaces.splice(idx, 1);
      await route.fulfill({ status: 204, body: "" });
    } else {
      await route.continue();
    }
  });
}

// ---------------------------------------------------------------------------
// User API keys mocking
// ---------------------------------------------------------------------------

export interface MockUserApiKey {
  id: string;
  client_id: string;
  name: string;
  scope: string[];
}

const DEFAULT_USER_KEYS: MockUserApiKey[] = [
  {
    id: "key-1",
    client_id: "sa-testuser-mykey",
    name: "my-key",
    scope: [],
  },
];

export async function mockUserKeysApi(
  page: Page,
  keys: MockUserApiKey[] = DEFAULT_USER_KEYS,
): Promise<void> {
  await page.route("**/api/user-keys", async (route) => {
    if (route.request().method() === "GET") {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ keys }),
      });
    } else if (route.request().method() === "POST") {
      const body = route.request().postDataJSON();
      const newKey: MockUserApiKey = {
        id: `key-${Date.now()}`,
        client_id: `sa-testuser-${body.name}`,
        name: body.name,
        scope: body.scope ?? [],
      };
      keys.push(newKey);
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
      });
    } else {
      await route.continue();
    }
  });

  await page.route("**/api/user-keys/*", async (route) => {
    if (route.request().method() === "DELETE") {
      const url = new URL(route.request().url());
      const id = url.pathname.split("/").pop();
      const idx = keys.findIndex((k) => k.id === id);
      if (idx >= 0) keys.splice(idx, 1);
      await route.fulfill({ status: 204, body: "" });
    } else {
      await route.continue();
    }
  });
}

// ---------------------------------------------------------------------------
// GitHub trust policies mocking
// ---------------------------------------------------------------------------

export interface MockGitHubTrustPolicy {
  id: string;
  owner_sub: string;
  name: string;
  repository: string;
  allowed_namespaces: string[];
  enabled: boolean;
  created_at: string;
  updated_at: string;
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
];

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
      });
      return;
    }
    if (route.request().method() === "POST") {
      const body = route.request().postDataJSON();
      const created: MockGitHubTrustPolicy = {
        id: `gh-policy-${Date.now()}`,
        owner_sub: "test-user-id",
        name: body.name,
        repository: body.repository,
        allowed_namespaces: body.allowed_namespaces ?? [],
        enabled: body.enabled ?? true,
        created_at: new Date().toISOString(),
        updated_at: new Date().toISOString(),
      };
      policies.unshift(created);
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      });
      return;
    }
    await route.continue();
  });

  await page.route("**/api/github-trust-policies/*", async (route) => {
    const url = new URL(route.request().url());
    const id = url.pathname.split("/").pop();
    const policy = policies.find((item) => item.id === id);
    if (route.request().method() === "PATCH") {
      if (!policy) {
        await route.fulfill({
          status: 404,
          body: JSON.stringify({ error: "not found" }),
        });
        return;
      }
      const body = route.request().postDataJSON();
      Object.assign(policy, body, { updated_at: new Date().toISOString() });
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify(policy),
      });
      return;
    }
    if (route.request().method() === "DELETE") {
      const idx = policies.findIndex((item) => item.id === id);
      if (idx >= 0) policies.splice(idx, 1);
      await route.fulfill({ status: 204, body: "" });
      return;
    }
    await route.continue();
  });
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
      ttlSecondsAfterCreated: 3600,
    },
    status: { replicas: 2, readyReplicas: 2 },
  };
  const template = {
    apiVersion: "osgym.cua.ai/v1alpha1",
    kind: "OSGymSandboxTemplate",
    metadata: { name: "demo-pool-template", namespace: "demo-pool" },
    spec: {
      vmTemplate: {
        containerDiskImage: "test-image:latest",
        cpuCores: 4,
        memory: "4Gi",
        services: [{ name: "mcp", targetPort: 8080, protocol: "TCP" }],
      },
    },
  };

  await page.route(
    "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/*/osgymsandboxwarmpools",
    async (route) => {
      const namespace = route
        .request()
        .url()
        .split("/namespaces/")[1]
        .split("/")[0];
      if (route.request().method() === "GET") {
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify({
            items: namespace === "demo-pool" ? [pool] : [],
          }),
        });
      } else if (route.request().method() === "POST") {
        const body = route.request().postDataJSON();
        await route.fulfill({
          status: 201,
          contentType: "application/json",
          body: JSON.stringify({
            ...body,
            status: { replicas: body.spec.replicas },
          }),
        });
      } else {
        await route.continue();
      }
    },
  );

  await page.route(
    "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/demo-pool/osgymsandboxwarmpools/demo-pool",
    async (route) => {
      if (route.request().method() === "GET") {
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify(pool),
        });
      } else if (route.request().method() === "DELETE") {
        await route.fulfill({ status: 204, body: "" });
      } else {
        await route.continue();
      }
    },
  );

  await page.route(
    "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/*/osgymsandboxtemplates",
    async (route) => {
      if (route.request().method() === "POST") {
        await route.fulfill({
          status: 201,
          contentType: "application/json",
          body: route.request().postData() ?? "{}",
        });
      } else {
        await route.continue();
      }
    },
  );

  await page.route(
    "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/demo-pool/osgymsandboxtemplates/demo-pool-template",
    async (route) => {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify(template),
      });
    },
  );
}

// ---------------------------------------------------------------------------
// Instances API mocking (OSGymSandbox CRD)
// ---------------------------------------------------------------------------

export async function mockInstancesApi(page: Page): Promise<void> {
  await page.route(
    "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/*/osgymsandboxes",
    async route => {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              apiVersion: "osgym.cua.ai/v1alpha1",
              kind: "OSGymSandbox",
              metadata: {
                name: "vm-xyz789",
                namespace: "demo-pool",
                creationTimestamp: "2026-05-28T09:55:00Z",
                annotations: {
                  "osgym.cua.ai/origin-warmpool": "demo-pool",
                  "osgym.cua.ai/origin-warmpool-namespace": "demo-pool",
                },
                ownerReferences: [
                  {
                    apiVersion: "osgym.cua.ai/v1alpha1",
                    kind: "OSGymSandboxClaim",
                    name: "claim-abc123",
                    uid: "claim-abc123-uid",
                    controller: true,
                  },
                ],
              },
              status: { phase: "Ready", ready: true },
            },
            {
              apiVersion: "osgym.cua.ai/v1alpha1",
              kind: "OSGymSandbox",
              metadata: {
                name: "vm-ready456",
                namespace: "demo-pool",
                creationTimestamp: "2026-05-28T09:58:00Z",
                labels: { "osgym.cua.ai/warmpool": "demo-pool" },
                annotations: {
                  "osgym.cua.ai/origin-warmpool": "demo-pool",
                  "osgym.cua.ai/origin-warmpool-namespace": "demo-pool",
                },
              },
              status: { phase: "Ready", ready: true },
            },
            {
              apiVersion: "osgym.cua.ai/v1alpha1",
              kind: "OSGymSandbox",
              metadata: {
                name: "other-pool-instance",
                namespace: "demo-pool",
                labels: { "osgym.cua.ai/warmpool": "other-pool" },
              },
              status: { phase: "Ready", ready: true },
            },
          ],
        }),
      })
    },
  )
}

export async function mockInstanceDetailApi(page: Page): Promise<void> {
  await page.route(
    "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/demo-pool/osgymsandboxes/vm-xyz789",
    async route => {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          apiVersion: "osgym.cua.ai/v1alpha1",
          kind: "OSGymSandbox",
          metadata: {
            name: "vm-xyz789",
            namespace: "demo-pool",
            creationTimestamp: "2026-05-28T09:55:00Z",
            annotations: {
              "osgym.cua.ai/origin-warmpool": "demo-pool",
              "osgym.cua.ai/origin-warmpool-namespace": "demo-pool",
            },
            ownerReferences: [
              {
                kind: "OSGymSandboxClaim",
                name: "claim-abc123",
                controller: true,
              },
            ],
          },
          spec: { vmTemplate: { runtime: "kubevirt" } },
          status: {
            phase: "Ready",
            ready: true,
            vmName: "vm-xyz789",
          },
        }),
      })
    },
  )

  await page.route(
    "**/api/k8s/api/v1/namespaces/demo-pool/pods?labelSelector=*",
    async route => {
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          items: [
            {
              metadata: {
                name: "virt-launcher-vm-xyz789-abcde",
                namespace: "demo-pool",
                creationTimestamp: "2026-05-28T09:55:10Z",
              },
              spec: {
                nodeName: "kubevirt-worker-1",
                containers: [{ name: "compute" }, { name: "guest-console-log" }],
              },
              status: { phase: "Running" },
            },
          ],
        }),
      })
    },
  )

  await page.route(
    "**/api/k8s/api/v1/namespaces/demo-pool/pods/virt-launcher-vm-xyz789-abcde/log?*",
    async route => {
      const url = new URL(route.request().url())
      const container = url.searchParams.get("container")
      if (container !== "guest-console-log") {
        await route.fulfill({ status: 400, body: "unexpected log container" })
        return
      }
      await route.fulfill({
        contentType: "text/plain",
        body: "2026-05-28T10:00:00Z guest console ready\n",
      })
    },
  )
}

// ---------------------------------------------------------------------------
// Claims API mocking (OSGymSandboxClaim CRD)
// ---------------------------------------------------------------------------

export interface MockClaim {
  name: string;
  namespace?: string;
  phase?: "Bound" | "Pending" | "Failed";
  sandboxName?: string;
  sandboxService?: string;
  ttlSecondsAfterCreated?: number;
}

export interface MockClaimsApiOptions {
  claims?: MockClaim[];
}

export interface MockClaimsApiControl {
  holdNextLists(count?: number): void;
  releaseLists(): void;
}

export async function mockClaimsApi(
  page: Page,
  options: MockClaimsApiOptions = {},
): Promise<MockClaimsApiControl> {
  const claims = options.claims ?? [
    {
      name: "claim-abc123",
      namespace: "demo-pool",
      phase: "Bound",
      sandboxName: "vm-xyz789",
      sandboxService: "vm-xyz789-svc",
      ttlSecondsAfterCreated: 1800,
    },
  ];
  const heldLists: Array<ReturnType<typeof deferred>> = [];
  const activeLists = new Set<ReturnType<typeof deferred>>();
  const items = claims.map((claim) => ({
    apiVersion: "osgym.cua.ai/v1alpha1",
    kind: "OSGymSandboxClaim",
    metadata: {
      name: claim.name,
      namespace: claim.namespace ?? "demo-pool",
      creationTimestamp: "2026-05-28T10:00:00Z",
    },
    spec: {
      sandboxTemplateRef: { name: "demo-pool-template" },
      warmpool: "demo-pool",
      ttlSecondsAfterCreated: claim.ttlSecondsAfterCreated,
    },
    status: {
      phase: claim.phase ?? "Bound",
      ...(claim.phase === "Pending" ? {} : {
        sandbox: {
          name: claim.sandboxName ?? `${claim.name}-sandbox`,
          service: claim.sandboxService ?? `${claim.name}-svc`,
        },
      }),
    },
  }));

  await page.route(
    "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/*/osgymsandboxclaims",
    async (route) => {
      if (route.request().method() === "GET") {
        const heldList = heldLists.shift();
        if (heldList) {
          activeLists.add(heldList);
          await heldList.promise;
          activeLists.delete(heldList);
        }
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify({ items }),
        });
      } else if (route.request().method() === "POST") {
        const body = route.request().postDataJSON();
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
        });
      } else {
        await route.continue();
      }
    },
  );

  // DELETE for individual claims
  await page.route(
    "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/*/osgymsandboxclaims/*",
    async (route) => {
      if (route.request().method() === "DELETE") {
        await route.fulfill({ status: 200, body: "{}" });
      } else {
        await route.continue();
      }
    },
  );

  return {
    holdNextLists(count = 1) {
      for (let index = 0; index < count; index += 1) heldLists.push(deferred());
    },
    releaseLists() {
      for (const list of [...heldLists.splice(0), ...activeLists]) list.resolve();
    },
  };
}

// ---------------------------------------------------------------------------
// Chat API mocking
// ---------------------------------------------------------------------------

export interface MockChatMessage {
  id: string;
  role: "user" | "assistant" | "tool";
  content: string;
  created_at: string;
  tool_call_id?: string;
  tool_calls?: Array<{
    id: string;
    type: "function";
    function: { name: string; arguments: string };
  }>;
}

export interface MockChatConversation {
  id: string;
  title: string;
  created_at: string;
  updated_at: string;
  archived_at?: string;
  messages: MockChatMessage[];
}

export interface MockChatApiOptions {
  conversations?: MockChatConversation[];
  holdList?: boolean;
  listError?: { message: string };
  listErrorAfterPatch?: { message: string };
  holdConversation?: boolean;
  turns?: MockChatTurn[];
  refreshError?: { afterTurnRequests: number; message: string };
  holdRefresh?: boolean;
  createError?: { message: string };
  holdPatch?: boolean;
  holdCreate?: boolean;
}

export interface MockChatApiControl {
  authorizationHeaders: string[];
  createRequests: number;
  listRequests: number;
  turnRequests: Array<{
    conversationId: string;
    messages: Array<Record<string, unknown>>;
  }>;
  patchRequests: Array<{ conversationId: string; archived: boolean }>;
  remainingTurns: number;
  releaseList(): void;
  releaseConversation(): void;
  releaseRefresh(): void;
  releaseTurn(): void;
  releasePatch(): void;
  releaseCreate(): void;
}

export interface MockChatTurn {
  events?: Array<Record<string, unknown>>;
  error?: { status: number; message: string };
  hold?: boolean;
  archiveBeforeError?: boolean;
}

function deferred(): { promise: Promise<void>; resolve: () => void } {
  let resolve = () => {};
  const promise = new Promise<void>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

function defaultChatConversations(): MockChatConversation[] {
  const now = new Date().toISOString();
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
    {
      id: "archived-conversation-1",
      title: "Archived browser task",
      created_at: now,
      updated_at: now,
      archived_at: now,
      messages: [],
    },
  ];
}

export async function mockChatApi(
  page: Page,
  options: MockChatApiOptions = {},
): Promise<MockChatApiControl> {
  const conversations = options.conversations ?? defaultChatConversations();
  const authorizationHeaders: string[] = [];
  const createRequests = { count: 0 };
  const listRequests = { count: 0 };
  const turnRequests: Array<{
    conversationId: string;
    messages: Array<Record<string, unknown>>;
  }> = [];
  const patchRequests: Array<{ conversationId: string; archived: boolean }> =
    [];
  const list = deferred();
  const conversation = deferred();
  const refresh = deferred();
  const turnGate = deferred();
  const patch = deferred();
  const create = deferred();
  const turns = [...(options.turns ?? [])];
  let refreshFailuresRemaining = options.refreshError ? 1 : 0;
  let postPatchListFailuresRemaining = options.listErrorAfterPatch ? 1 : 0;
  let createFailuresRemaining = options.createError ? 1 : 0;

  if (!options.holdList) list.resolve();
  if (!options.holdConversation) conversation.resolve();
  if (!options.holdRefresh) refresh.resolve();
  if (!turns.some((item) => item.hold)) turnGate.resolve();
  if (!options.holdPatch) patch.resolve();
  if (!options.holdCreate) create.resolve();

  await page.route(
    (url) => url.pathname === "/api/chat/conversations",
    async (route) => {
      if (route.request().method() === "GET") {
        listRequests.count += 1;
        authorizationHeaders.push(
          route.request().headers().authorization ?? "",
        );
        if (options.listError) {
          await route.fulfill({
            status: 500,
            contentType: "application/json",
            body: JSON.stringify({ error: options.listError.message }),
          });
          return;
        }
        if (
          options.listErrorAfterPatch &&
          patchRequests.length > 0 &&
          postPatchListFailuresRemaining > 0
        ) {
          postPatchListFailuresRemaining -= 1;
          await route.fulfill({
            status: 500,
            contentType: "application/json",
            body: JSON.stringify({ error: options.listErrorAfterPatch.message }),
          });
          return;
        }
        if (
          options.refreshError &&
          turnRequests.length >= options.refreshError.afterTurnRequests &&
          refreshFailuresRemaining > 0
        ) {
          refreshFailuresRemaining -= 1;
          await route.fulfill({
            status: 500,
            contentType: "application/json",
            body: JSON.stringify({ error: options.refreshError.message }),
          });
          return;
        }
        const archivedValues = new URL(route.request().url()).searchParams.getAll("archived");
        let archived = false;
        if (archivedValues.length === 1 && archivedValues[0] === "true") archived = true;
        else if (archivedValues.length !== 0 && (archivedValues.length !== 1 || archivedValues[0] !== "false")) {
          await route.fulfill({
            status: 400,
            contentType: "application/json",
            body: JSON.stringify({ error: "invalid archived query value" }),
          });
          return;
        }
        const summaries = conversations
          .filter((item) => Boolean(item.archived_at) === archived)
          .map(({ messages: _messages, ...summary }) => summary);
        await list.promise;
        if (options.holdRefresh && turnRequests.length > 0)
          await refresh.promise;
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify(summaries),
        });
        return;
      }

      if (route.request().method() === "POST") {
        createRequests.count += 1;
        await create.promise;
        if (createFailuresRemaining > 0) {
          createFailuresRemaining -= 1;
          await route.fulfill({
            status: 500,
            contentType: "application/json",
            body: JSON.stringify({ error: options.createError!.message }),
          });
          return;
        }
        const now = new Date().toISOString();
        const created: MockChatConversation = {
          id: `conversation-${conversations.length + 1}`,
          title: "",
          created_at: now,
          updated_at: now,
          messages: [],
        };
        conversations.unshift(created);
        await route.fulfill({
          status: 201,
          contentType: "application/json",
          body: JSON.stringify(created),
        });
        return;
      }

      await route.continue();
    },
  );

  await page.route("**/api/chat/conversations/*", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const pathParts = url.pathname.split("/").filter(Boolean);
    if (
      pathParts.length !== 4 ||
      pathParts.slice(0, 3).join("/") !== "api/chat/conversations"
    ) {
      await route.fallback();
      return;
    }

    const id = pathParts[3];
    const selected = conversations.find((item) => item.id === id);
    if (!selected) {
      await route.fulfill({
        status: 404,
        contentType: "application/json",
        body: JSON.stringify({ error: "conversation not found" }),
      });
      return;
    }

    if (request.method() === "PATCH") {
      let body: unknown;
      try {
        body = JSON.parse(request.postData() ?? "");
      } catch {
        await route.fulfill({
          status: 400,
          contentType: "application/json",
          body: JSON.stringify({ error: "invalid body" }),
        });
        return;
      }
      if (typeof body !== "object" || body === null || Array.isArray(body)) {
        await route.fulfill({
          status: 400,
          contentType: "application/json",
          body: JSON.stringify({ error: "invalid body" }),
        });
        return;
      }
      const archiveRequest = body as Record<string, unknown>;
      const fields = Object.keys(archiveRequest);
      if (
        fields.length !== 1 ||
        fields[0] !== "archived" ||
        typeof archiveRequest.archived !== "boolean"
      ) {
        await route.fulfill({
          status: 400,
          contentType: "application/json",
          body: JSON.stringify({ error: "invalid body" }),
        });
        return;
      }
      patchRequests.push({ conversationId: id, archived: archiveRequest.archived });
      await patch.promise;
      const now = new Date().toISOString();
      selected.updated_at = now;
      if (archiveRequest.archived) selected.archived_at = now;
      else delete selected.archived_at;
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify(selected),
      });
      return;
    }

    if (request.method() !== "GET") {
      await route.fallback();
      return;
    }

    if (
      options.refreshError &&
      turnRequests.length >= options.refreshError.afterTurnRequests &&
      refreshFailuresRemaining > 0
    ) {
      refreshFailuresRemaining -= 1;
      await route.fulfill({
        status: 500,
        contentType: "application/json",
        body: JSON.stringify({ error: options.refreshError.message }),
      });
      return;
    }
    await conversation.promise;
    if (options.holdRefresh && turnRequests.length > 0) await refresh.promise;
    await route.fulfill({
      contentType: "application/json",
      body: JSON.stringify(selected),
    });
  });

  await page.route("**/api/chat/conversations/*/turns", async (route) => {
    if (route.request().method() !== "POST") {
      await route.continue();
      return;
    }

    const url = new URL(route.request().url());
    const conversationId = url.pathname.split("/").slice(-2, -1)[0];
    const body = route.request().postDataJSON() as {
      messages?: Array<Record<string, unknown>>;
    };
    turnRequests.push({ conversationId, messages: body.messages ?? [] });
    const now = new Date().toISOString();
    const selected = conversations.find((item) => item.id === conversationId);
    if (selected?.archived_at) {
      await route.fulfill({
        status: 409,
        contentType: "application/json",
        body: JSON.stringify({ error: "conversation is archived" }),
      });
      return;
    }
    if (selected) {
      for (const message of body.messages ?? []) {
        selected.messages.push({
          id: `message-${selected.messages.length + 1}`,
          role: message.role as MockChatMessage["role"],
          content: String(message.content ?? ""),
          created_at: now,
          ...(typeof message.tool_call_id === "string"
            ? { tool_call_id: message.tool_call_id }
            : {}),
        });
      }
      selected.updated_at = now;
    }
    const turn = turns.shift();
    if (!turn) {
      await route.fulfill({
        status: 500,
        contentType: "application/json",
        body: JSON.stringify({ error: "No mock turn" }),
      });
      return;
    }
    if (turn.hold) await turnGate.promise;
    if (turn.archiveBeforeError && selected) selected.archived_at = now;
    if (turn.error) {
      await route.fulfill({
        status: turn.error.status,
        contentType: "application/json",
        body: JSON.stringify({ error: turn.error.message }),
      });
      return;
    }
    if (selected) {
      const final = [...(turn.events ?? [])]
        .reverse()
        .find((event) => event.type === "assistant")?.message as
        Record<string, unknown> | undefined;
      if (final)
        selected.messages.push({
          id: `message-${selected.messages.length + 1}`,
          role: "assistant",
          content: String(final.content ?? ""),
          created_at: now,
          ...(Array.isArray(final.tool_calls)
            ? { tool_calls: final.tool_calls as MockChatMessage["tool_calls"] }
            : {}),
        });
    }
    await route.fulfill({
      contentType: "application/x-ndjson",
      body: (turn.events ?? [])
        .map((event) => JSON.stringify(event) + "\n")
        .join(""),
    });
  });

  return {
    authorizationHeaders,
    get createRequests() {
      return createRequests.count;
    },
    get listRequests() {
      return listRequests.count;
    },
    get remainingTurns() {
      return turns.length;
    },
    turnRequests,
    patchRequests,
    releaseList: list.resolve,
    releaseConversation: conversation.resolve,
    releaseRefresh: refresh.resolve,
    releaseTurn: turnGate.resolve,
    releasePatch: patch.resolve,
    releaseCreate: create.resolve,
  };
}

// ---------------------------------------------------------------------------
// Signed service URLs API mocking
// ---------------------------------------------------------------------------

export interface MockSignedServiceUrl {
  id: string;
  namespace: string;
  claim: string;
  sandbox: string;
  service: string;
  logicalService: string;
  label?: string;
  url: string;
  createdAt: string;
  expiresAt: string;
  revokedAt?: string;
}

export interface MockSignedServiceUrlsApiOptions {
  holdListClaims?: string[];
  holdCreateClaims?: string[];
  holdRevokeIds?: string[];
  unavailable?: { list?: boolean; create?: boolean; revoke?: boolean };
}

export async function mockSignedServiceUrlsApi(
  page: Page,
  initial: MockSignedServiceUrl[] = [],
  options: MockSignedServiceUrlsApiOptions = {},
): Promise<{
  urls: MockSignedServiceUrl[];
  requests: Array<{ method: string; id?: string; body?: Record<string, unknown> }>;
  releaseList(claim: string): void;
  releaseCreate(claim: string): void;
  releaseRevoke(id: string): void;
}> {
  const urls = [...initial];
  const requests: Array<{ method: string; id?: string; body?: Record<string, unknown> }> = [];
  const heldLists = new Map<string, ReturnType<typeof deferred>>(
    (options.holdListClaims ?? []).map((claim) => [claim, deferred()]),
  );
  const heldCreates = new Map<string, ReturnType<typeof deferred>>(
    (options.holdCreateClaims ?? []).map((claim) => [claim, deferred()]),
  );
  const heldRevokes = new Map<string, ReturnType<typeof deferred>>(
    (options.holdRevokeIds ?? []).map((id) => [id, deferred()]),
  );

  await page.route(/\/api\/signed-service-urls\/[^/?]+(?:\/[^/?]+)?(?:\?.*)?$/, async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const parts = url.pathname.split("/").filter(Boolean);
    const namespace = decodeURIComponent(parts[2] ?? "");
    const id = parts.length > 3 ? decodeURIComponent(parts[3]) : undefined;
    const body = request.postData()
      ? request.postDataJSON() as Record<string, unknown>
      : undefined;
    requests.push({ method: request.method(), id, body });

    if (request.method() === "GET" && !id) {
      const claim = url.searchParams.get("claim") ?? "";
      await heldLists.get(claim)?.promise;
      if (options.unavailable?.list) {
        await route.fulfill({
          status: 503,
          contentType: "application/json",
          body: JSON.stringify({ error: "signed service URLs are unavailable" }),
        });
        return;
      }
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify(urls.filter((item) => item.namespace === namespace && item.claim === claim)),
      });
      return;
    }

    if (request.method() === "POST" && !id && body) {
      await heldCreates.get(String(body.claim))?.promise;
      if (options.unavailable?.create) {
        await route.fulfill({
          status: 503,
          contentType: "application/json",
          body: JSON.stringify({ error: "signed service URLs are unavailable" }),
        });
        return;
      }
      const signedUrlId = `00000000-0000-4000-8000-${String(urls.length + 1).padStart(12, "0")}`;
      const createdAt = "2099-01-01T00:00:00Z";
      const created: MockSignedServiceUrl = {
        id: signedUrlId,
        namespace,
        claim: String(body.claim),
        sandbox: String(body.sandbox),
        service: String(body.service),
        logicalService: String(body.logicalService),
        ...(typeof body.label === "string" ? { label: body.label } : {}),
        url: `https://run.cua.ai/api/signed/${signedUrlId}`,
        createdAt,
        expiresAt: new Date(Date.parse(createdAt) + Number(body.expiresInSeconds) * 1000).toISOString(),
      };
      urls.push(created);
      await route.fulfill({
        status: 201,
        contentType: "application/json",
        body: JSON.stringify(created),
      });
      return;
    }

    if (request.method() === "DELETE" && id) {
      await heldRevokes.get(id)?.promise;
      if (options.unavailable?.revoke) {
        await route.fulfill({
          status: 503,
          contentType: "application/json",
          body: JSON.stringify({ error: "signed service URLs are unavailable" }),
        });
        return;
      }
      const index = urls.findIndex((item) => item.namespace === namespace && item.id === id);
      if (index < 0) {
        await route.fulfill({
          status: 404,
          contentType: "application/json",
          body: JSON.stringify({ error: "signed URL not found" }),
        });
        return;
      }
      urls[index].revokedAt = "2026-08-31T12:30:00Z";
      await route.fulfill({ status: 204, body: "" });
      return;
    }

    await route.continue();
  });

  return {
    urls,
    requests,
    releaseList(claim: string) {
      heldLists.get(claim)?.resolve();
    },
    releaseCreate(claim: string) {
      heldCreates.get(claim)?.resolve();
    },
    releaseRevoke(id: string) {
      heldRevokes.get(id)?.resolve();
    },
  };
}
