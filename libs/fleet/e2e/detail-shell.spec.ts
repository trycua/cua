import { expect, test } from "@playwright/test"
import {
  mockAuth,
  mockClaimsApi,
  mockInstancesApi,
  mockNamespacesApi,
  mockPoolsApi,
} from "./fixtures/mock-api"
import { expectSharedPageShell } from "./fixtures/shell-geometry"

async function mockFleet(page: Parameters<typeof mockAuth>[0]) {
  await mockAuth(page)
  await mockNamespacesApi(page)
  await mockPoolsApi(page)
  await mockInstancesApi(page)
  await mockClaimsApi(page)
}

for (const viewport of [
  { name: "desktop", width: 1440, height: 900 },
  { name: "mobile", width: 375, height: 900 },
]) {
  test(`pool detail uses the shared shell on ${viewport.name}`, async ({ page }) => {
    await page.setViewportSize(viewport)
    await mockFleet(page)
    await page.goto("/pools/demo-pool/demo-pool")

    await expect(page.getByRole("heading", { name: "demo-pool", level: 1 })).toBeVisible()
    await expect(page.getByRole("heading", { name: "Instances" })).toBeVisible()
    await expectSharedPageShell(page)
  })

  test(`claim detail uses the shared shell on ${viewport.name}`, async ({ page }) => {
    await page.setViewportSize(viewport)
    await mockFleet(page)
    await page.goto(
      "/pools/demo-pool/demo-pool/claims/claim-abc123",
    )

    await expect(page.getByRole("heading", { name: "claim-abc123", level: 1 })).toBeVisible()
    await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible()
    await expect(page.getByText("vm-xyz789", { exact: true })).toBeVisible()
    await expectSharedPageShell(page)
  })
}
