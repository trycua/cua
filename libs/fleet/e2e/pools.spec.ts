import { test, expect } from "@playwright/test"
import {
  mockAuth,
  mockNamespacesApi,
  mockPoolsApi,
  mockClaimsApi,
} from "./fixtures/mock-api"

test.describe("Pool creation", () => {
  test("pool creation form has name field", async ({ page }) => {
    await mockAuth(page)
    await mockNamespacesApi(page)
    await mockPoolsApi(page)

    await page.goto("/pools/new")

    // The form should have a name input
    await expect(page.getByPlaceholder("my-pool")).toBeVisible()
  })

  test("form has all required fields", async ({ page }) => {
    await mockAuth(page)
    await mockNamespacesApi(page)
    await mockPoolsApi(page)

    await page.goto("/pools/new")

    // Name input
    await expect(page.getByPlaceholder("my-pool")).toBeVisible()
    // Numeric inputs (vCPU, replicas)
    await expect(page.getByRole("spinbutton").first()).toBeVisible()

    // Services section has an "Add service" button
    await expect(page.getByRole("button", { name: "Add service" })).toBeVisible()
  })
})

test.describe("Pool duplication", () => {
  // Reproduces the bug: clicking "Duplicate" for a pool in the list should
  // open the New pool form pre-filled with that pool's existing config. The
  // list row only carries summary fields (name, replicas, phase,
  // availableCount), so PoolNew's `source.cpu ?? DEFAULTS.cpu` (and ram /
  // ociImage / services) all fall through to DEFAULTS — the form shows a
  // fresh "new pool" with default data instead of the source's config.
  test("duplicate from the list pre-fills the form with the source pool's config", async ({
    page,
  }) => {
    await mockAuth(page)
    await mockNamespacesApi(page)

    // Source pool with a NON-default resource spec (defaults are cpu 4,
    // ram 4Gi, the ECR image, replicas 1, no services).
    const sourcePool = {
      apiVersion: "osgym.cua.ai/v1alpha1",
      kind: "OSGymSandboxWarmPool",
      metadata: { name: "demo-pool", namespace: "demo-pool" },
      spec: {
        replicas: 3,
        sandboxTemplateRef: { name: "demo-pool-template" },
      },
      status: { replicas: 3, readyReplicas: 3 },
    }
    const sourceTemplate = {
      apiVersion: "osgym.cua.ai/v1alpha1",
      kind: "OSGymSandboxTemplate",
      metadata: { name: "demo-pool-template", namespace: "demo-pool" },
      spec: {
        vmTemplate: {
          containerDiskImage: "custom-image:v1",
          cpuCores: 8,
          memory: "16Gi",
          services: [{ name: "api", targetPort: 8080, protocol: "TCP" }],
        },
      },
    }

    // listPools() iterates the user's namespaces and lists pools in each.
    await page.route(
      "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/*/osgymsandboxwarmpools",
      async (route) => {
        if (route.request().method() !== "GET") return route.continue()
        const ns = route.request().url().split("/namespaces/")[1].split("/")[0]
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify({ items: ns === "demo-pool" ? [sourcePool] : [] }),
        })
      },
    )

    // Single-pool GET — where a correct duplicate flow would read the full
    // spec. Registered after the list route so it wins for this exact path.
    await page.route(
      "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/demo-pool/osgymsandboxwarmpools/demo-pool",
      async (route) => {
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify(sourcePool),
        })
      },
    )
    await page.route(
      "**/api/k8s/apis/osgym.cua.ai/v1alpha1/namespaces/demo-pool/osgymsandboxtemplates/demo-pool-template",
      async (route) => {
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify(sourceTemplate),
        })
      },
    )

    await page.goto("/pools")

    // Select the pool's row and click Duplicate.
    const row = page.getByRole("row").filter({ hasText: "demo-pool" })
    await expect(row).toBeVisible()
    await row.getByRole("checkbox").check()
    await page.getByRole("button", { name: "Duplicate" }).click()

    // We land on the duplicate form...
    await expect(
      page.getByRole("heading", { name: "Duplicate pool" }),
    ).toBeVisible()
    // ...with the name seeded from the source (this part already works).
    await expect(page.getByRole("textbox", { name: "Name" })).toHaveValue(
      "demo-pool-copy",
    )

    // The bug: these reflect DEFAULTS instead of the source pool's config.
    await expect(
      page.getByRole("spinbutton", { name: "vCPU cores" }),
    ).toHaveValue("8")
    await expect(page.getByRole("textbox", { name: "RAM" })).toHaveValue("16Gi")
    await expect(page.getByRole("textbox", { name: "OCI image" })).toHaveValue(
      "custom-image:v1",
    )
  })
})

test.describe("Pool detail with claims", () => {
  test("pool detail page shows claims section", async ({ page }) => {
    await mockAuth(page)
    await mockNamespacesApi(page)
    await mockPoolsApi(page)
    await mockClaimsApi(page)

    await page.goto("/pools/demo-pool/demo-pool")

    // The pool name should be shown
    await expect(page.getByRole("heading", { name: "demo-pool" })).toBeVisible()

    // The Claims section should be present
    await expect(page.getByRole("heading", { name: "Claims" })).toBeVisible()

    // The "Create claim" button should be visible
    await expect(
      page.getByRole("button", { name: "Create claim" }),
    ).toBeVisible()

    // The existing mock claim should be listed
    const claimRow = page.getByRole("row").filter({ hasText: "claim-abc123" })
    await expect(claimRow).toBeVisible()
    await expect(page.getByText("Bound")).toBeVisible()
    await expect(claimRow.getByRole("cell").nth(3)).not.toHaveText("-")
  })
})

test.describe("Retired navigation", () => {
  test("does not expose removed admin and runtime surfaces", async ({ page }) => {
    await mockAuth(page)
    await mockNamespacesApi(page)
    await mockPoolsApi(page)

    await page.goto("/pools")

    await expect(page.getByRole("link", { name: "Nodes" })).toHaveCount(0)
    await expect(page.getByRole("link", { name: "Operator events" })).toHaveCount(0)
    await expect(page.getByRole("link", { name: "API keys", exact: true })).toHaveCount(0)
    await expect(page.getByRole("link", { name: "User API keys" })).toBeVisible()
    await expect(page.getByRole("link", { name: "Settings" })).toBeVisible()

    for (const route of ["/nodes", "/operator-events", "/operator-logs", "/api-keys"]) {
      await page.goto(route)
      await expect(
        page.getByRole("heading", { name: /nodes|events|logs|api keys/i }),
      ).toHaveCount(0)
    }
  })
})
