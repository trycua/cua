import { test, expect } from "@playwright/test"
import {
  mockAuth,
  mockNamespacesApi,
  mockGitHubTrustPoliciesApi,
} from "./fixtures/mock-api"

test.describe("Settings GitHub trust policies", () => {
  test.beforeEach(async ({ page }) => {
    await mockAuth(page)
    await mockNamespacesApi(page)
    await mockGitHubTrustPoliciesApi(page)
  })

  test("renders existing trust policies and workflow guidance", async ({ page }) => {
    await page.goto("/settings")

    await expect(
      page.getByRole("heading", { name: "GitHub Actions OIDC" }),
    ).toBeVisible()
    await expect(page.getByText("cloud-ci")).toBeVisible()
    await expect(page.getByText("trycua/cloud")).toBeVisible()
    await expect(page.getByText("https://token.actions.githubusercontent.com")).toBeVisible()
    await expect(page.getByText("permissions:")).toBeVisible()
  })

  test("can create, edit, disable, and delete a trust policy", async ({ page }) => {
    await page.goto("/settings")

    await page.getByLabel("Display name").fill("preview-ci")
    await page.getByLabel("Repository").fill("trycua/preview")
    await expect(page.getByLabel("Additional namespaces")).toHaveCount(0)
    await page.getByRole("button", { name: "Create policy" }).click()

    await expect(page.getByText("preview-ci")).toBeVisible()
    await expect(page.getByText("trycua/preview")).toBeVisible()

    await page.getByRole("button", { name: "Edit" }).nth(1).click()
    await page.getByLabel("Display name").fill("preview-ci-updated")
    await page.getByRole("button", { name: "Save policy" }).click()
    await expect(page.getByText("preview-ci-updated")).toBeVisible()

    await page.getByRole("button", { name: "Disable" }).nth(0).click()
    await expect(page.getByText("No").first()).toBeVisible()

    await page.getByRole("button", { name: "Delete" }).nth(1).click()
    await page.getByRole("button", { name: "Delete" }).last().click()
    await expect(page.getByText("preview-ci-updated")).toHaveCount(0)
  })
})
