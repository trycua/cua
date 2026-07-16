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
      metadata: { name: "demo-pool", namespace: "my-workspace" },
      spec: {
        replicas: 3,
        template: {
          containerDiskImage: "custom-image:v1",
          cpuCores: 8,
          memory: "16Gi",
        },
        services: [{ name: "api", targetPort: 8080, protocol: "TCP" }],
      },
      status: { phase: "Ready", totalCount: 3, availableCount: 3, claimedCount: 0 },
    }

    // listPools() iterates the user's namespaces and lists pools in each.
    await page.route(
      "**/api/k8s/apis/cua.ai/v1/namespaces/*/osgymworkspacepools",
      async (route) => {
        if (route.request().method() !== "GET") return route.continue()
        const ns = route.request().url().split("/namespaces/")[1].split("/")[0]
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify({ items: ns === "my-workspace" ? [sourcePool] : [] }),
        })
      },
    )

    // Single-pool GET — where a correct duplicate flow would read the full
    // spec. Registered after the list route so it wins for this exact path.
    await page.route(
      "**/api/k8s/apis/cua.ai/v1/namespaces/my-workspace/osgymworkspacepools/demo-pool",
      async (route) => {
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify(sourcePool),
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

    // Mock the getPool endpoint for a specific pool
    await page.route(
      "**/api/k8s/apis/cua.ai/v1/namespaces/my-workspace/osgymworkspacepools/demo-pool",
      async (route) => {
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify({
            metadata: { name: "demo-pool", namespace: "my-workspace" },
            spec: {
              replicas: 2,
              template: {
                containerDiskImage: "test-image:latest",
                cpuCores: 4,
                memory: "4Gi",
              },
              services: [],
            },
            status: {
              phase: "Ready",
              totalCount: 2,
              availableCount: 1,
              claimedCount: 1,
            },
          }),
        })
      },
    )

    await page.goto("/pools/my-workspace/demo-pool")

    // The pool name should be shown
    await expect(page.getByRole("heading", { name: "demo-pool" })).toBeVisible()

    // The Claims section should be present
    await expect(page.getByRole("heading", { name: "Claims" })).toBeVisible()

    // The "Create claim" button should be visible
    await expect(
      page.getByRole("button", { name: "Create claim" }),
    ).toBeVisible()

    // The existing mock claim should be listed
    await expect(page.getByText("claim-abc123")).toBeVisible()
    await expect(page.getByText("Bound")).toBeVisible()
  })
})
