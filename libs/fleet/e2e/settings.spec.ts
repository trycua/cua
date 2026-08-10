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

    const githubToggle = page.getByRole("button", {
      name: "GitHub Actions OIDC",
    })
    await expect(githubToggle).toBeVisible()
    await githubToggle.click()
    await expect(page.getByText("cloud-ci")).toBeVisible()
    await expect(page.getByText("trycua/cloud")).toBeVisible()
    await expect(page.getByText("https://token.actions.githubusercontent.com")).toBeVisible()
    await expect(page.getByText("permissions:")).toBeVisible()
    await expect(page.getByText("fleets", { exact: true })).toBeVisible()
    await expect(page.getByText("Workflow job snippet", { exact: true })).toBeVisible()
    const workflowSnippet = page.locator("pre").filter({
      hasText: "Get GitHub WIF token for Fleets",
    })
    await expect(workflowSnippet).toContainText(
      "pip install cua-cli==0.1.14",
    )
    await expect(workflowSnippet).not.toContainText("--extra-index-url")
    await expect(workflowSnippet).toContainText('token="$(cua wif-token github)"')
    await expect(workflowSnippet).toContainText('echo "::add-mask::$token"')
    await expect(workflowSnippet).toContainText(
      'printf \'token=%s\\n\' "$token" >> "$GITHUB_OUTPUT"',
    )
    await expect(workflowSnippet).toContainText(
      'sandbox="replace-with-an-authorized-namespace"',
    )
    await expect(workflowSnippet).toContainText(
      "This exact value is authorized by the repository trust policy.",
    )
    await expect(workflowSnippet).toContainText("FLEETS_TOKEN")
    await expect(workflowSnippet).toContainText("Get GitHub WIF token for Fleets")
    await expect(workflowSnippet).toContainText("status=$?")
    await expect(workflowSnippet).toContainText("trap cleanup EXIT")
    await expect(workflowSnippet).toContainText(
      'cua sb delete --force "$sandbox"',
    )
    await expect(workflowSnippet).toContainText(
      'if [ "$status" -eq 0 ]; then status=1; fi',
    )
    await expect(workflowSnippet).toContainText('exit "$status"')
    await expect(workflowSnippet).toContainText(
      "296062593712.dkr.ecr.us-west-2.amazonaws.com/desktop-workspace:latest",
    )
    await expect(workflowSnippet).toContainText("cua sb launch")
    await expect(workflowSnippet).toContainText('--name "$sandbox"')
    await expect(workflowSnippet).toContainText(
      "cua sb exec \"$sandbox\" sh -lc 'uname -a; id; pwd'",
    )
    await expect(workflowSnippet).not.toContainText("curl")
    await expect(workflowSnippet).not.toContainText("/api/namespaces")
    await expect(workflowSnippet).not.toContainText("--namespace")
    await expect(workflowSnippet).not.toContainText(
      "ACTIONS_ID_TOKEN_REQUEST_URL",
    )
    await expect(workflowSnippet).not.toContainText(
      "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
    )
    await expect(workflowSnippet).not.toContainText("CYCLOPS_CS_TOKEN")
    await expect(workflowSnippet).not.toContainText("CYCLOPS_TOKEN")
    await expect(workflowSnippet).not.toContainText("${GITHUB_RUN_ID}")
    await expect(workflowSnippet).not.toContainText("${GITHUB_RUN_ATTEMPT}")

    const workflowSnippetText = await workflowSnippet.textContent()
    expect(
      workflowSnippetText?.match(
        /sandbox="replace-with-an-authorized-namespace"/g,
      ) ?? [],
    ).toHaveLength(1)
  })

  test("shows GitHub WIF below payment method in a collapsed section", async ({
    page,
  }) => {
    await page.route("**/api/config", route =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ admin: false, billing: true }),
      }),
    )
    await page.route("**/api/billing/summary", route =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ payment_method_present: false, card: null }),
      }),
    )

    await page.goto("/settings")

    const paymentHeading = page.getByRole("heading", {
      name: "Payment method",
      exact: true,
    })
    const githubToggle = page.getByRole("button", {
      name: "GitHub Actions OIDC",
    })
    await expect(paymentHeading).toBeVisible()
    await expect(githubToggle).toBeVisible()
    const paymentBeforeGitHub = await page.evaluate(() => {
      const payment = [...document.querySelectorAll("h1, h2, h3, h4")].find(
        element => element.textContent?.trim() === "Payment method",
      )
      const github = [...document.querySelectorAll("button, [role=\"button\"]")].find(element =>
        element.textContent?.trim().startsWith("GitHub Actions OIDC"),
      )
      return Boolean(
        payment &&
          github &&
          payment.compareDocumentPosition(github) & Node.DOCUMENT_POSITION_FOLLOWING,
      )
    })
    expect(paymentBeforeGitHub).toBe(true)
    await expect(page.getByLabel("Display name")).toBeHidden()

    await githubToggle.click()
    await expect(page.getByLabel("Display name")).toBeVisible()
  })

  test("can create, edit, disable, and delete a trust policy", async ({ page }) => {
    await page.goto("/settings")
    await page.getByRole("button", { name: "GitHub Actions OIDC" }).click()

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
