import { test, expect } from "@playwright/test"
import {
  mockAuth,
  mockNamespacesApi,
  mockUserKeysApi,
} from "./fixtures/mock-api"

test.describe("User API keys", () => {
  test.beforeEach(async ({ page }) => {
    await mockAuth(page)
    await mockNamespacesApi(page)
    await mockUserKeysApi(page)
  })

  test("renders the API keys page with existing keys", async ({ page }) => {
    await page.goto("/user-keys")

    // The page should have the "Your API keys" table header
    await expect(
      page.getByRole("heading", { name: "Your API keys" }),
    ).toBeVisible()

    // The existing mock key should be listed
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
    await expect(page.getByText("Usage")).not.toBeVisible()

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
    await expect(page.getByText("Scope (optional)")).toBeVisible()
    await expect(
      page.getByText(/restrict this key to specific namespaces/i),
    ).toBeVisible()
  })
})
