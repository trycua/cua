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
    await expect(page.getByText("fleets", { exact: true })).toBeVisible()
    await expect(page.getByText("Workflow job snippet", { exact: true })).toBeVisible()
    const workflowSnippet = page.locator("pre").filter({
      hasText: "Get GitHub WIF token for Fleets",
    })
    await expect(workflowSnippet).toContainText("pip install cua-cli==0.1.13")
    await expect(workflowSnippet).toContainText('token="$(cua wif-token github)"')
    await expect(workflowSnippet).toContainText(
      'printf \'token=%s\\n\' "$token" >> "$GITHUB_OUTPUT"',
    )
    await expect(workflowSnippet).toContainText("curl --fail-with-body -sSL")
    await expect(workflowSnippet).toContainText(
      "https://run.cua.ai/api/namespaces",
    )
    await expect(workflowSnippet).toContainText("FLEETS_TOKEN")
    await expect(workflowSnippet).toContainText("Get GitHub WIF token for Fleets")
    await expect(workflowSnippet).not.toContainText(
      "ACTIONS_ID_TOKEN_REQUEST_URL",
    )
    await expect(workflowSnippet).not.toContainText("CYCLOPS_CS_TOKEN")
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
