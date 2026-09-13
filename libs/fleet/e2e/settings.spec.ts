import { test, expect } from "@playwright/test"
import {
  mockAuth,
  mockNamespacesApi,
  mockGitHubTrustPoliciesApi,
} from "./fixtures/mock-api"
import { expectSharedPageShell } from "./fixtures/shell-geometry"

for (const viewport of [
  { name: "desktop", width: 1440, height: 900 },
  { name: "mobile", width: 375, height: 900 },
]) {
  test(`Settings uses the shared shell on ${viewport.name}`, async ({ page }) => {
    await page.setViewportSize(viewport)
    await mockAuth(page, { billing: true })
    await mockNamespacesApi(page)
    await mockGitHubTrustPoliciesApi(page)
    await page.route("**/api/billing/summary", route =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ payment_method_present: false, card: null }),
      }),
    )

    await page.goto("/settings")

    await expectSharedPageShell(page)
    await expect(page).toHaveTitle("Settings · Cua")
    await expect(page.getByLabel("Display name")).toBeHidden()
    expect(
      await page.locator("html").evaluate(
        element => element.scrollWidth - element.clientWidth,
      ),
    ).toBeLessThanOrEqual(1)
  })
}

test("keeps account identity in a collapsed drawer", async ({ page }) => {
  await mockAuth(page)
  await mockNamespacesApi(page)
  await mockGitHubTrustPoliciesApi(page)
  await page.goto("/settings")

  const accountToggle = page.getByRole("button", { name: "Account" })
  const settingsContent = page.getByRole("main")
  await expect(accountToggle).toHaveAttribute("aria-expanded", "false")
  await expect(settingsContent.getByText("testuser", { exact: true })).toBeHidden()
  await expect(settingsContent.getByText("test-user-id", { exact: true })).toBeHidden()

  await accountToggle.click()

  await expect(accountToggle).toHaveAttribute("aria-expanded", "true")
  await expect(settingsContent.getByText("testuser", { exact: true })).toBeVisible()
  await expect(settingsContent.getByText("test-user-id", { exact: true })).toBeVisible()
})

test("switches the shared UI language and persists it", async ({ page }) => {
  await mockAuth(page)
  await mockNamespacesApi(page)
  await mockGitHubTrustPoliciesApi(page)
  await page.goto("/settings")

  await page.getByLabel("Display language").click()
  await page.getByRole("option", { name: "Español" }).click()

  await expect(page).toHaveTitle("Configuración · Cua")
  await expect(page.locator("html")).toHaveAttribute("lang", "es")
  await expect(page.getByRole("heading", { name: "Idioma y región" })).toBeVisible()
  await expect(page.getByRole("link", { name: "Grupos" })).toBeVisible()
  await expect(page.getByRole("link", { name: "Uso" })).toBeVisible()
  await expect(page.getByRole("link", { name: "Claves de API" })).toBeVisible()
  await expect(page.getByRole("link", { name: "Configuración" })).toBeVisible()

  await page.reload()
  await expect(page).toHaveTitle("Configuración · Cua")
  await expect(page.getByLabel("Idioma de la interfaz")).toContainText("Español")
})

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
      "public.ecr.aws/k5j5w0x5/cua-ubuntu-24.04:latest",
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

  test("orders account, payment method, and GitHub OIDC", async ({
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
    const accountHeading = page.getByRole("heading", {
      name: "Account",
      exact: true,
    })
    const githubToggle = page.getByRole("button", {
      name: "GitHub Actions OIDC",
    })
    await expect(page.getByRole("heading", { name: "Settings" })).toBeVisible()
    await expect(accountHeading).toBeVisible()
    await expect(paymentHeading).toBeVisible()
    await expect(githubToggle).toBeVisible()
    await expect(
      page.getByRole("heading", { name: "API keys", exact: true }),
    ).toHaveCount(0)
    const sectionOrder = await page.evaluate(() => {
      const elements = [...document.querySelectorAll("h1, h2, h3, h4, button")]
      const indexOf = (label: string) =>
        elements.findIndex(element => element.textContent?.trim().startsWith(label))
      return [
        indexOf("Account"),
        indexOf("Payment method"),
        indexOf("GitHub Actions OIDC"),
      ]
    })
    expect(sectionOrder.every(index => index >= 0)).toBe(true)
    expect(sectionOrder).toEqual([...sectionOrder].sort((a, b) => a - b))
    await expect(page.getByLabel("Display name")).toBeHidden()

    await githubToggle.click()
    await expect(page.getByLabel("Display name")).toBeVisible()
  })

  test("keeps account usable when GitHub settings fail", async ({ page }) => {
    await page.unroute("**/api/github-trust-policies")
    await page.route("**/api/github-trust-policies", route =>
      route.fulfill({ status: 503, body: "temporarily unavailable" }),
    )

    await page.goto("/settings")
    await expect(page.getByRole("heading", { name: "Account" })).toBeVisible()
    await expect(
      page.getByText("GitHub Actions settings are unavailable. Expand to retry."),
    ).toBeVisible()
    await page.getByRole("button", { name: "GitHub Actions OIDC" }).click()
    await expect(
      page.getByRole("heading", { name: "GitHub Actions settings are unavailable" }),
    ).toBeVisible()
    await expect(page.getByRole("button", { name: "Retry" })).toBeVisible()
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
    await page
      .getByRole("dialog")
      .getByRole("button", { name: "Delete" })
      .click()
    await expect(page.getByText("preview-ci-updated")).toHaveCount(0)
  })
})
