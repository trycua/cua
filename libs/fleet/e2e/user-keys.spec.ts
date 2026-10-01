import { test, expect } from "@playwright/test"
import {
  mockAuth,
  mockNamespacesApi,
  mockUserKeysApi,
} from "./fixtures/mock-api"
import { expectSharedPageShell } from "./fixtures/shell-geometry"

for (const viewport of [
  { name: "desktop", width: 1440, height: 900 },
  { name: "mobile", width: 375, height: 900 },
]) {
  test(`User API keys uses the shared shell on ${viewport.name}`, async ({ page }) => {
    await page.setViewportSize(viewport)
    await mockAuth(page)
    await mockNamespacesApi(page)
    await mockUserKeysApi(page)

    await page.goto("/user-keys")

    await expectSharedPageShell(page)
    await expect(page.getByRole("button", { name: "Create key" })).toBeVisible()
    expect(
      await page.locator("html").evaluate(
        element => element.scrollWidth - element.clientWidth,
      ),
    ).toBeLessThanOrEqual(1)
  })
}

test("User API keys reflows at 200% zoom and 320 CSS pixels", async ({ page }) => {
  await page.setViewportSize({ width: 640, height: 900 })
  await mockAuth(page)
  await mockNamespacesApi(page)
  await mockUserKeysApi(page)
  await page.goto("/user-keys")

  await page.locator("html").evaluate(element => {
    element.style.zoom = "200%"
  })
  expect(
    await page.locator("html").evaluate(
      element => element.scrollWidth - element.clientWidth,
    ),
  ).toBeLessThanOrEqual(1)

  await page.locator("html").evaluate(element => {
    element.style.zoom = ""
  })
  await page.setViewportSize({ width: 320, height: 900 })
  await expect(page.getByRole("button", { name: "Create key" })).toBeVisible()
  expect(
    await page.locator("html").evaluate(
      element => element.scrollWidth - element.clientWidth,
    ),
  ).toBeLessThanOrEqual(1)
})

test.describe("User API keys", () => {
  test.beforeEach(async ({ page }) => {
    await mockAuth(page)
    await mockNamespacesApi(page)
    await mockUserKeysApi(page)
  })

  test("renders the API keys page with existing keys", async ({ page }) => {
    await page.goto("/user-keys")

    // The shared page shell and key table should both be visible. Page
    // title (h1) and the key-table's own Header (h2) both read "API keys"
    // now, so disambiguate by heading level.
    await expect(
      page.getByRole("heading", { name: "API keys", exact: true, level: 1 }),
    ).toBeVisible()

    // The existing mock key should be listed
    await expect(page.getByText("my-key")).toBeVisible()
    await expect(page.getByText("sa-testuser-mykey")).toBeVisible()
  })

  test("renders keys when the backend returns a null scope", async ({ page }) => {
    await page.unroute("**/api/user-keys")
    await page.route("**/api/user-keys", (route) =>
      route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({
          keys: [
            {
              id: "key-1",
              client_id: "sa-testuser-mykey",
              name: "my-key",
              scope: null,
            },
          ],
        }),
      }),
    )

    await page.goto("/user-keys")

    await expect(page.getByText("Error: SdkError.Body")).not.toBeVisible()
    await expect(page.getByText("my-key")).toBeVisible()
    await expect(page.getByText("sa-testuser-mykey")).toBeVisible()
  })

  test("shows the create key form", async ({ page }) => {
    await page.goto("/user-keys")

    // Wait for the page heading to confirm the route rendered
    await expect(
      page.getByRole("heading", { name: /api keys/i }).first(),
    ).toBeVisible({ timeout: 10000 })

    // The create key button should be visible
    await expect(
      page.getByRole("button", { name: /create/i }).first(),
    ).toBeVisible()
  })

  test("can create a new API key and see credentials", async ({ page }) => {
    await page.goto("/user-keys")

    // Wait for page to load
    await expect(
      page.getByRole("heading", { name: /api keys/i }).first(),
    ).toBeVisible({ timeout: 10000 })

    // Fill in the name — find the first text input on the page
    const nameInput = page.getByRole("textbox").first()
    await nameInput.fill("test-key")

    // Click create
    await page.getByRole("button", { name: "Create key" }).click()

    // The credentials modal should appear with client_id and client_secret
    await expect(page.getByText("API key created")).toBeVisible()
    await expect(page.getByText("sa-testuser-test-key").first()).toBeVisible()
    await expect(
      page.getByText("fake-client-secret-shown-once").first(),
    ).toBeVisible()

    await expect(page.getByText("Token URL")).not.toBeVisible()
    await expect(page.getByRole("link", { name: "Usage", exact: true })).toBeVisible()

    // Dismiss the modal
    await page
      .getByRole("button", { name: "I have copied the credentials" })
      .click()
  })

  test("has revoke button for each key", async ({ page }) => {
    await page.goto("/user-keys")

    // Each key should have a Revoke button (multiple keys = multiple buttons)
    await expect(page.getByRole("button", { name: "Revoke" }).first()).toBeVisible()
  })

  test("shows revoke confirmation modal", async ({ page }) => {
    await page.goto("/user-keys")

    await page.getByRole("button", { name: "Revoke" }).first().click()

    // Confirmation modal should appear
    await expect(page.getByText("Revoke API key?")).toBeVisible()
    await expect(
      page.getByText(/this action cannot be undone/i),
    ).toBeVisible()

    // Should have Cancel and Revoke buttons in the modal
    await expect(page.getByRole("button", { name: "Cancel" })).toBeVisible()
  })

  test("shows scope field with namespace options", async ({ page }) => {
    await page.goto("/user-keys")

    // The scope multiselect should mention namespaces
    await expect(page.getByText("Allowed Namespaces (optional)")).toBeVisible()
  })

  test("filters namespace options by typed text", async ({ page }) => {
    await page.goto("/user-keys")

    await page
      .getByText("All namespaces (no restriction)", { exact: true })
      .click()
    await page.getByPlaceholder("Filter namespaces").fill("stag")

    await expect(page.getByRole("option", { name: "staging" })).toBeVisible()
    await expect(page.getByRole("option", { name: "demo-pool" })).toHaveCount(0)
  })

  test("shows a retryable table error without hiding the create form", async ({ page }) => {
    await page.unroute("**/api/user-keys")
    await page.route("**/api/user-keys", route =>
      route.fulfill({ status: 503, body: "temporarily unavailable" }),
    )

    await page.goto("/user-keys")

    await expect(page.getByRole("heading", { name: "Create API key" })).toBeVisible()
    await expect(page.getByText("API keys are unavailable")).toBeVisible()
    await expect(page.getByRole("button", { name: "Retry" })).toBeVisible()
  })
})
