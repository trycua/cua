import { expect, test } from "@playwright/test"
import {
  DEFAULT_ADMIN_FEATURE_FLAGS,
  mockAdminFeatureFlagsApi,
  mockAuth,
  mockConfigApi,
  mockNamespacesApi,
  mockPoolsApi,
  type MockAdminFeatureFlag,
} from "./fixtures/mock-api"

test.describe("feature flag admin route", () => {
  test("redirects malformed deep links instead of rendering an empty shell", async ({ page }) => {
    await mockAuth(page, { admin: true })
    await mockNamespacesApi(page)
    await mockPoolsApi(page)

    await page.goto("/admin/feature-flags.")

    await expect(page).toHaveURL(/\/pools$/)
    await expect(page.getByRole("heading", { name: "Pools", level: 1 })).toBeVisible()
  })

  test("shows the admin navigation and page after admin config resolves", async ({
    page,
  }) => {
    await mockAuth(page)
    await mockNamespacesApi(page)
    await mockPoolsApi(page)

    let resolveConfig: ((value: void) => void) | undefined
    const configReady = new Promise<void>(resolve => {
      resolveConfig = resolve
    })
    await page.route("**/api/config", async route => {
      await configReady
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ admin: true, billing: false }),
      })
    })

    await page.goto("/pools")
    await expect(page.getByRole("link", { name: "Feature flags" })).toHaveCount(0)

    resolveConfig?.()
    await expect(page.getByRole("link", { name: "Feature flags" })).toBeVisible()
    await expect(page.getByText("Admin", { exact: true })).toBeVisible()
    await page.getByRole("link", { name: "Feature flags" }).click()
    await expect(page).toHaveURL(/\/admin\/feature-flags$/)
    await expect(page.getByRole("heading", { name: "Feature flags" })).toBeVisible()
    await expect(page.getByText("Admin only · Feature flagged", { exact: true })).toBeVisible()
    await expect(page).toHaveTitle("Feature flags · Cua")
  })

  test("redirects non-admin direct navigation without requesting admin flags", async ({
    page,
  }) => {
    await mockAuth(page)
    await mockNamespacesApi(page)
    await mockPoolsApi(page)

    let resolveConfig: (() => void) | undefined
    const configReady = new Promise<void>(resolve => {
      resolveConfig = resolve
    })
    await page.route("**/api/config", async route => {
      await configReady
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ admin: false, billing: false }),
      })
    })

    const adminRequests: string[] = []
    await page.route("**/api/admin/feature-flags**", async route => {
      adminRequests.push(route.request().url())
      await route.abort("failed")
    })

    await page.goto("/admin/feature-flags")
    await expect(page.getByRole("heading", { name: "Feature flags" })).toHaveCount(0)

    resolveConfig?.()
    await expect(page).toHaveURL(/\/pools$/)
    await page.waitForTimeout(250)
    await expect(page.getByRole("heading", { name: "Feature flags" })).toHaveCount(0)
    expect(adminRequests).toEqual([])
  })
})

async function openAdminFeatureFlags(page: import("@playwright/test").Page): Promise<void> {
  await mockAuth(page)
  await mockConfigApi(page, { admin: true })
  await page.goto("/admin/feature-flags")
  await expect(page.getByRole("heading", { name: "Feature flags" })).toBeVisible()
}

function featureFlagRow(page: import("@playwright/test").Page, key: string) {
  return page.getByRole("row").filter({ has: page.getByText(key, { exact: true }) })
}

test.describe("feature flags CRUD", () => {
  test("lists, filters, refreshes, and protects managed flags", async ({ page }) => {
    const api = await mockAdminFeatureFlagsApi(page)
    await openAdminFeatureFlags(page)

    const terraformRow = featureFlagRow(page, "admin-subs")
    await expect(terraformRow).toContainText('["test-user-id","other-admin"]')
    await expect(terraformRow).toContainText("JSON")
    await expect(terraformRow).toContainText("Terraform")
    await expect(terraformRow).toContainText("7")
    await expect(terraformRow).toContainText("Aug 12, 2026")
    await expect(terraformRow.getByRole("link", { name: "Terraform" })).toHaveAttribute(
      "href",
      /github\.com\/trycua\/cloud\/tree\/main\/terraform\/aws\/feature-flags/,
    )

    const adHocRow = featureFlagRow(page, "new-checkout")
    await expect(adHocRow).toContainText("true")
    await expect(adHocRow).toContainText("Boolean")
    await expect(adHocRow).toContainText("Ad hoc")
    await expect(featureFlagRow(page, "legacy-threshold")).toContainText("External")

    await page.getByPlaceholder("Find feature flags").fill("checkout")
    await expect(adHocRow).toBeVisible()
    await expect(featureFlagRow(page, "admin-subs")).toHaveCount(0)
    await page.getByRole("button", { name: "Clear filter" }).click()

    api.flags.push({
      key: "refreshed-flag",
      path: "/feature-flags/cyclops-cs/refreshed-flag",
      value_type: "string",
      value: "ready",
      raw_value: "ready",
      ownership: "ad_hoc",
      managed_by: "cyclops-cs",
      deletable: true,
      version: 1,
      modified_at: "2026-08-13T12:10:00Z",
      tags: {},
    })
    await page.getByRole("button", { name: "Refresh feature flags" }).click()
    await expect(featureFlagRow(page, "refreshed-flag")).toBeVisible()

    await terraformRow.getByRole("button", { name: "Actions for admin-subs" }).click()
    await expect(page.getByRole("menuitem", { name: "Delete" })).toHaveCount(0)
    await page.keyboard.press("Escape")
    await featureFlagRow(page, "legacy-threshold").getByRole("button", { name: "Actions for legacy-threshold" }).click()
    await expect(page.getByRole("menuitem", { name: "Delete" })).toHaveCount(0)
  })

  test("never offers deletion for admin-subs even when ownership metadata is wrong", async ({ page }) => {
    const unsafeAdminSubs: MockAdminFeatureFlag = {
      ...DEFAULT_ADMIN_FEATURE_FLAGS[0],
      ownership: "ad_hoc",
      managed_by: "cyclops-cs-admin",
      deletable: true,
    }
    await mockAdminFeatureFlagsApi(page, [unsafeAdminSubs])
    await openAdminFeatureFlags(page)

    await featureFlagRow(page, "admin-subs").getByRole("button", { name: "Actions for admin-subs" }).click()
    await expect(page.getByRole("menuitem", { name: "Delete" })).toHaveCount(0)
  })

  test("renders row actions above the table overflow boundary", async ({ page }) => {
    await mockAdminFeatureFlagsApi(page)
    await openAdminFeatureFlags(page)

    await featureFlagRow(page, "new-checkout").getByRole("button", { name: "Actions for new-checkout" }).click()
    const deleteAction = page.getByRole("menuitem", { name: "Delete" })
    const deleteActionBox = await deleteAction.boundingBox()

    expect(deleteActionBox).not.toBeNull()
    const centerIsInteractive = await page.evaluate(({ x, y }) => {
      return document.elementFromPoint(x, y)?.closest('[role="menuitem"]') !== null
    }, {
      x: deleteActionBox!.x + deleteActionBox!.width / 2,
      y: deleteActionBox!.y + deleteActionBox!.height / 2,
    })
    expect(centerIsInteractive).toBe(true)
  })

  test("creates every typed value and validates malformed JSON", async ({ page }) => {
    const api = await mockAdminFeatureFlagsApi(page, [])
    await openAdminFeatureFlags(page)

    const cases = [
      { key: "bool-flag", type: "Boolean", apiType: "boolean", value: "true", expected: true },
      { key: "number-flag", type: "Number", apiType: "number", value: "42.5", expected: 42.5 },
      { key: "string-flag", type: "String", apiType: "string", value: "hello", expected: "hello" },
      { key: "json-flag", type: "JSON", apiType: "json", value: '{"rollout":[1,2]}', expected: { rollout: [1, 2] } },
    ] as const

    for (const [index, testCase] of cases.entries()) {
      await page.getByRole("button", { name: "Create feature flag" }).click()
      const editor = page.getByRole("dialog", { name: "Create feature flag" })
      await editor.getByRole("textbox", { name: "Key" }).fill(testCase.key)
      if (index === 0) {
        await editor.getByRole("textbox", { name: "Description (optional)" }).fill("Boolean rollout")
      }
      await editor.getByLabel("Value type").click()
      await page.getByRole("option", { name: testCase.type, exact: true }).click()
      if (testCase.type === "Boolean") {
        await editor.getByRole("button", { name: /^Value Value / }).click()
        await page.getByRole("option", { name: "True", exact: true }).click()
      } else if (testCase.type === "JSON") {
        await editor.getByRole("textbox", { name: "Value" }).fill("{")
        await page.getByRole("button", { name: "Create", exact: true }).click()
        await expect(editor.locator("span").filter({ hasText: /^Enter valid JSON$/ })).toBeVisible()
        expect(api.requests.filter(request => request.method === "POST")).toHaveLength(index)
        await editor.getByRole("textbox", { name: "Value" }).fill(testCase.value)
      } else {
        await editor.getByRole(testCase.type === "Number" ? "spinbutton" : "textbox", { name: "Value" }).fill(testCase.value)
      }
      await page.getByRole("button", { name: "Create", exact: true }).click()
      await expect(page.getByText(`Created ${testCase.key}`)).toBeVisible()
      await expect(featureFlagRow(page, testCase.key)).toBeVisible()
    }

    expect(api.requests.filter(request => request.method === "POST").map(request => request.body)).toEqual(
      cases.map((testCase, index) => ({
        key: testCase.key,
        value_type: testCase.apiType,
        value: testCase.expected,
        ...(index === 0 ? { description: "Boolean rollout" } : {}),
      })),
    )
  })

  test("shows stored descriptions and keeps them create-only", async ({ page }) => {
    await mockAdminFeatureFlagsApi(page)
    await openAdminFeatureFlags(page)

    await featureFlagRow(page, "new-checkout").getByRole("button", { name: "Actions for new-checkout" }).click()
    await page.getByRole("menuitem", { name: "Edit" }).click()
    const editor = page.getByRole("dialog", { name: "Edit new-checkout" })
    await expect(editor.getByText("Enable the checkout experiment")).toBeVisible()
    await expect(editor.getByRole("textbox", { name: "Description (optional)" })).toHaveCount(0)
  })

  test("refreshes access and redirects on middleware not_admin", async ({ page }) => {
    let configRequests = 0
    await mockAuth(page)
    await page.route("**/api/config", route => {
      configRequests += 1
      return route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ admin: configRequests === 1, billing: false }),
      })
    })
    await mockNamespacesApi(page)
    await mockPoolsApi(page)
    await page.route("**/api/admin/feature-flags", route => route.fulfill({
      status: 403,
      contentType: "application/json",
      body: JSON.stringify({ code: "not_admin", message: "feature flag administration requires an admin user" }),
    }))

    await page.goto("/admin/feature-flags")
    await expect(page).toHaveURL(/\/pools$/)
    await expect(page.getByRole("link", { name: "Feature flags" })).toHaveCount(0)
    expect(configRequests).toBeGreaterThanOrEqual(2)
  })

  test("handles stable middleware authorization errors", async ({ page }) => {
    await mockAuth(page)
    await mockConfigApi(page, { admin: true })
    await page.route("**/api/admin/feature-flags", route => route.fulfill({
      status: 502,
      contentType: "application/json",
      body: JSON.stringify({ code: "authorization_unavailable", message: "authorization check unavailable" }),
    }))
    await page.goto("/admin/feature-flags")
    await expect(page.getByText("authorization check unavailable")).toBeVisible()
  })

  test("edits type with an expected version and reloads after a conflict", async ({ page }) => {
    const stale = { ...DEFAULT_ADMIN_FEATURE_FLAGS[1], version: 3 }
    const current: MockAdminFeatureFlag = {
      ...stale,
      value: false,
      raw_value: "false",
      version: 4,
      modified_at: "2026-08-13T11:00:00Z",
    }
    const putBodies: Record<string, unknown>[] = []
    await mockAdminFeatureFlagsApi(page, [stale])
    await page.route("**/api/admin/feature-flags/new-checkout", async route => {
      if (route.request().method() !== "PUT") {
        await route.fallback()
        return
      }
      const body = route.request().postDataJSON() as Record<string, unknown>
      putBodies.push(body)
      if (putBodies.length === 1) {
        await route.fulfill({
          status: 409,
          contentType: "application/json",
          body: JSON.stringify({ code: "version_conflict", message: "feature flag version is stale", current }),
        })
        return
      }
      await route.fallback()
    })
    await openAdminFeatureFlags(page)

    await featureFlagRow(page, "new-checkout").getByRole("button", { name: "Actions for new-checkout" }).click()
    await page.getByRole("menuitem", { name: "Edit" }).click()
    const editor = page.getByRole("dialog", { name: "Edit new-checkout" })
    await editor.getByLabel("Value type").click()
    await page.getByRole("option", { name: "Number", exact: true }).click()
    await editor.getByRole("spinbutton", { name: "Value" }).fill("9")
    await page.getByRole("button", { name: "Save changes" }).click()

    await expect(page.getByText("This flag changed after you loaded it.")).toBeVisible()
    await page.getByRole("button", { name: "Reload current value" }).click()
    await expect(editor.getByLabel("Value type")).toContainText("Boolean")
    await expect(editor.getByRole("button", { name: /^Value Value / })).toContainText("False")
    await editor.getByRole("button", { name: /^Value Value / }).click()
    await page.getByRole("option", { name: "True", exact: true }).click()
    await page.getByRole("button", { name: "Save changes" }).click()
    await expect(page.getByText("Updated new-checkout")).toBeVisible()

    expect(putBodies).toEqual([
      { value_type: "number", value: 9, expected_version: 3 },
      { value_type: "boolean", value: true, expected_version: 4 },
    ])
  })

  test("requires an exact key before deleting an ad hoc flag", async ({ page }) => {
    const api = await mockAdminFeatureFlagsApi(page)
    await openAdminFeatureFlags(page)

    await featureFlagRow(page, "new-checkout").getByRole("button", { name: "Actions for new-checkout" }).click()
    await page.getByRole("menuitem", { name: "Delete" }).click()
    const deleteDialog = page.getByRole("dialog", { name: "Delete new-checkout?" })
    await expect(deleteDialog).toBeVisible()
    await expect(
      deleteDialog.getByText("Deleting this flag causes consumers to receive defaults after the feature flag cache refreshes."),
    ).toBeVisible()
    const confirm = page.getByLabel("Type new-checkout to confirm")
    await confirm.fill("new-checkou")
    await expect(page.getByRole("button", { name: "Delete feature flag" })).toBeDisabled()
    await confirm.fill("new-checkout")
    await page.getByRole("button", { name: "Delete feature flag" }).click()
    await expect(featureFlagRow(page, "new-checkout")).toHaveCount(0)
    expect(api.requests.find(request => request.method === "DELETE")?.body).toEqual({ expected_version: 3 })
  })

  test("confirms self-removal then refreshes access and redirects", async ({ page }) => {
    let configRequests = 0
    await mockAuth(page)
    await page.route("**/api/config", route => {
      configRequests += 1
      return route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ admin: configRequests === 1, billing: false }),
      })
    })
    const api = await mockAdminFeatureFlagsApi(page, [DEFAULT_ADMIN_FEATURE_FLAGS[0]])
    await mockNamespacesApi(page)
    await mockPoolsApi(page)
    await page.goto("/admin/feature-flags")

    await featureFlagRow(page, "admin-subs").getByRole("button", { name: "Actions for admin-subs" }).click()
    await page.getByRole("menuitem", { name: "Edit" }).click()
    await page.getByRole("dialog", { name: "Edit admin-subs" }).getByRole("textbox", { name: "Value" }).fill('["other-admin"]')
    await page.getByRole("button", { name: "Save changes" }).click()
    await expect(page.getByText("You are removing your own administrator access.")).toBeVisible()
    await expect(page.getByRole("button", { name: "Confirm and save" })).toBeDisabled()
    await page.getByLabel("I understand I will lose access").check()
    await page.getByRole("button", { name: "Confirm and save" }).click()

    await expect(page).toHaveURL(/\/pools$/)
    await expect(page.getByRole("link", { name: "Feature flags" })).toHaveCount(0)
    expect(api.requests.find(request => request.method === "PUT")?.body).toEqual({
      value_type: "json",
      value: ["other-admin"],
      expected_version: 7,
    })
    expect(configRequests).toBe(2)
    const updateIndex = api.requests.findIndex(request => request.method === "PUT")
    expect(updateIndex).toBeGreaterThanOrEqual(0)
    expect(api.requests.slice(updateIndex + 1).filter(request => request.method === "GET")).toHaveLength(0)
  })
  test("shows an initial list failure", async ({ page }) => {
    await mockAuth(page)
    await mockConfigApi(page, { admin: true })
    await page.route("**/api/admin/feature-flags", route =>
      route.fulfill({
        status: 503,
        contentType: "application/json",
        body: JSON.stringify({ code: "unavailable", message: "feature flags are unavailable" }),
      }),
    )

    await page.goto("/admin/feature-flags")

    await expect(page.getByText("feature flags are unavailable")).toBeVisible()
  })

  test("preserves existing rows when manual refresh fails", async ({ page }) => {
    await mockAdminFeatureFlagsApi(page)
    await openAdminFeatureFlags(page)
    let listRequests = 0
    await page.route("**/api/admin/feature-flags", async route => {
      if (route.request().method() !== "GET") {
        await route.fallback()
        return
      }
      listRequests += 1
      await route.fulfill({
        status: 503,
        contentType: "application/json",
        body: JSON.stringify({ code: "refresh_failed", message: "refresh failed" }),
      })
    })

    await page.getByRole("button", { name: "Refresh feature flags" }).click()

    await expect(page.getByText("refresh failed")).toBeVisible()
    await expect(featureFlagRow(page, "new-checkout")).toBeVisible()
    expect(listRequests).toBe(1)
  })

  test("rejects oversized keys and scalar JSON before calling the API", async ({ page }) => {
    const api = await mockAdminFeatureFlagsApi(page, [])
    await openAdminFeatureFlags(page)

    await page.getByRole("button", { name: "Create feature flag" }).click()
    let editor = page.getByRole("dialog", { name: "Create feature flag" })
    await editor.getByRole("textbox", { name: "Key" }).fill("a".repeat(64))
    await page.getByRole("button", { name: "Create", exact: true }).click()
    await expect(editor.getByText("Feature flag keys can be at most 63 characters")).toBeVisible()
    expect(api.requests.filter(request => request.method === "POST")).toHaveLength(0)

    await editor.getByRole("button", { name: "Cancel" }).click()
    await expect(editor).toBeHidden()
    await page.getByLabel("Feature flags", { exact: true }).getByRole("button", { name: "Create feature flag" }).click()
    editor = page.getByRole("dialog", { name: "Create feature flag" })
    await editor.getByRole("textbox", { name: "Key" }).fill("json-scalar")
    await editor.getByLabel("Value type").click()
    await page.getByRole("option", { name: "JSON", exact: true }).click()
    for (const scalar of ["null", "true", "42", '"text"']) {
      await editor.getByRole("textbox", { name: "Value" }).fill(scalar)
      await page.getByRole("button", { name: "Create", exact: true }).click()
      await expect(editor.getByText("JSON values must be an object or array")).toBeVisible()
    }
    expect(api.requests.filter(request => request.method === "POST")).toHaveLength(0)
  })

  test("edits a Terraform-managed flag without exposing deletion", async ({ page }) => {
    const api = await mockAdminFeatureFlagsApi(page, [DEFAULT_ADMIN_FEATURE_FLAGS[0]])
    await openAdminFeatureFlags(page)

    await featureFlagRow(page, "admin-subs").getByRole("button", { name: "Actions for admin-subs" }).click()
    await expect(page.getByRole("menuitem", { name: "Delete" })).toHaveCount(0)
    await page.getByRole("menuitem", { name: "Edit" }).click()
    const editor = page.getByRole("dialog", { name: "Edit admin-subs" })
    await expect(editor.getByText("The key is protected; only its typed value can be changed.")).toBeVisible()
    await editor.getByRole("textbox", { name: "Value" }).fill('["test-user-id","other-admin","third-admin"]')
    await page.getByRole("button", { name: "Save changes" }).click()

    await expect(page.getByText("Updated admin-subs")).toBeVisible()
    expect(api.requests.find(request => request.method === "PUT")?.body).toEqual({
      value_type: "json",
      value: ["test-user-id", "other-admin", "third-admin"],
      expected_version: 7,
    })
  })

  test("provides an accessible full-value view", async ({ page }) => {
    await mockAdminFeatureFlagsApi(page)
    await openAdminFeatureFlags(page)

    await featureFlagRow(page, "admin-subs").getByRole("button", { name: "View full value for admin-subs" }).click()

    const details = page.getByRole("dialog", { name: "Value for admin-subs" })
    await expect(details).toContainText('"test-user-id"')
    await expect(details.getByRole("button", { name: "Copy full value" })).toBeVisible()
  })

  test("keeps delete failures local and recovers from stale versions", async ({ page }) => {
    const current = { ...DEFAULT_ADMIN_FEATURE_FLAGS[1], version: 4, value: false, raw_value: "false" }
    await mockAdminFeatureFlagsApi(page, [DEFAULT_ADMIN_FEATURE_FLAGS[1]])
    let deleteRequests = 0
    await page.route("**/api/admin/feature-flags/new-checkout", async route => {
      if (route.request().method() !== "DELETE") {
        await route.fallback()
        return
      }
      deleteRequests += 1
      if (deleteRequests === 1) {
        await route.fulfill({
          status: 409,
          contentType: "application/json",
          body: JSON.stringify({ code: "version_conflict", message: "feature flag version is stale", current }),
        })
        return
      }
      if (deleteRequests === 2) {
        await route.fulfill({
          status: 503,
          contentType: "application/json",
          body: JSON.stringify({ code: "delete_failed", message: "delete failed" }),
        })
        return
      }
      await route.fallback()
    })
    await openAdminFeatureFlags(page)

    await featureFlagRow(page, "new-checkout").getByRole("button", { name: "Actions for new-checkout" }).click()
    await page.getByRole("menuitem", { name: "Delete" }).click()
    let dialog = page.getByRole("dialog", { name: "Delete new-checkout?" })
    await dialog.getByLabel("Type new-checkout to confirm").fill("new-checkout")
    await dialog.getByRole("button", { name: "Delete feature flag" }).click()
    await expect(dialog.getByText("This flag changed after you loaded it.")).toBeVisible()
    await dialog.getByRole("button", { name: "Reload current version" }).click()
    await expect(dialog.getByText("Current version: 4")).toBeVisible()
    await dialog.getByLabel("Type new-checkout to confirm").fill("new-checkout")
    await dialog.getByRole("button", { name: "Delete feature flag" }).click()
    await expect(dialog.getByText("delete failed")).toBeVisible()
    await expect(featureFlagRow(page, "new-checkout")).toBeVisible()
  })

  test("keeps self-removal failures inside the confirmation modal", async ({ page }) => {
    await mockAdminFeatureFlagsApi(page, [DEFAULT_ADMIN_FEATURE_FLAGS[0]])
    await page.route("**/api/admin/feature-flags/admin-subs", async route => {
      if (route.request().method() === "PUT") {
        await route.fulfill({
          status: 503,
          contentType: "application/json",
          body: JSON.stringify({ code: "update_failed", message: "self-removal update failed" }),
        })
        return
      }
      await route.fallback()
    })
    await openAdminFeatureFlags(page)

    await featureFlagRow(page, "admin-subs").getByRole("button", { name: "Actions for admin-subs" }).click()
    await page.getByRole("menuitem", { name: "Edit" }).click()
    await page.getByRole("dialog", { name: "Edit admin-subs" }).getByRole("textbox", { name: "Value" }).fill('["other-admin"]')
    await page.getByRole("button", { name: "Save changes" }).click()
    const confirmation = page.getByRole("dialog", { name: "Confirm loss of administrator access" })
    await confirmation.getByLabel("I understand I will lose access").check()
    await confirmation.getByRole("button", { name: "Confirm and save" }).click()

    await expect(confirmation.getByText("self-removal update failed")).toBeVisible()
    await expect(confirmation).toBeVisible()
  })

  test("returns self-removal version conflicts to the editor", async ({ page }) => {
    const current = {
      ...DEFAULT_ADMIN_FEATURE_FLAGS[0],
      value: ["test-user-id", "other-admin", "new-admin"],
      raw_value: '["test-user-id","other-admin","new-admin"]',
      version: 8,
    }
    await mockAdminFeatureFlagsApi(page, [DEFAULT_ADMIN_FEATURE_FLAGS[0]])
    await page.route("**/api/admin/feature-flags/admin-subs", async route => {
      if (route.request().method() === "PUT") {
        await route.fulfill({
          status: 409,
          contentType: "application/json",
          body: JSON.stringify({ code: "version_conflict", message: "feature flag version is stale", current }),
        })
        return
      }
      await route.fallback()
    })
    await openAdminFeatureFlags(page)

    await featureFlagRow(page, "admin-subs").getByRole("button", { name: "Actions for admin-subs" }).click()
    await page.getByRole("menuitem", { name: "Edit" }).click()
    await page.getByRole("dialog", { name: "Edit admin-subs" }).getByRole("textbox", { name: "Value" }).fill('["other-admin"]')
    await page.getByRole("button", { name: "Save changes" }).click()
    const confirmation = page.getByRole("dialog", { name: "Confirm loss of administrator access" })
    await confirmation.getByLabel("I understand I will lose access").check()
    await confirmation.getByRole("button", { name: "Confirm and save" }).click()

    await expect(confirmation).toBeHidden()
    const editor = page.getByRole("dialog", { name: "Edit admin-subs" })
    await expect(editor.getByText("This flag changed after you loaded it.")).toBeVisible()
    await editor.getByRole("button", { name: "Reload current value" }).click()
    await expect(editor.getByText("Current version: 8")).toBeVisible()
    await expect(editor.getByRole("textbox", { name: "Value" })).toHaveValue(
      '[\n  "test-user-id",\n  "other-admin",\n  "new-admin"\n]',
    )
  })

  test("sorts feature flags by key", async ({ page }) => {
    await mockAdminFeatureFlagsApi(page)
    await openAdminFeatureFlags(page)

    const keyHeader = page.getByRole("columnheader", { name: /Key/ })
    await keyHeader.getByRole("button").click()
    let keys = await page.getByRole("row").locator("th code").allTextContents()
    expect(keys).toEqual(["admin-subs", "legacy-threshold", "new-checkout"])

    await keyHeader.getByRole("button").click()
    keys = await page.getByRole("row").locator("th code").allTextContents()
    expect(keys).toEqual(["new-checkout", "legacy-threshold", "admin-subs"])
  })

  test("uses multiline string editing and explains versions and managed behavior", async ({ page }) => {
    const stringFlag: MockAdminFeatureFlag = {
      key: "welcome-message",
      path: "/feature-flags/cyclops-cs/welcome-message",
      value_type: "string",
      value: "hello\nworld",
      raw_value: "hello\nworld",
      ownership: "ad_hoc",
      managed_by: "cyclops-cs",
      deletable: true,
      version: 5,
      modified_at: "2026-08-13T12:00:00Z",
      tags: {},
    }
    await mockAdminFeatureFlagsApi(page, [stringFlag, DEFAULT_ADMIN_FEATURE_FLAGS[2]])
    await openAdminFeatureFlags(page)

    await featureFlagRow(page, "welcome-message").getByRole("button", { name: "Actions for welcome-message" }).click()
    await page.getByRole("menuitem", { name: "Edit" }).click()
    let editor = page.getByRole("dialog", { name: "Edit welcome-message" })
    await expect(editor.getByRole("textbox", { name: "Value" })).toHaveJSProperty("tagName", "TEXTAREA")
    await expect(editor.getByText("Current version: 5")).toBeVisible()
    await editor.getByRole("button", { name: "Cancel" }).click()

    await featureFlagRow(page, "legacy-threshold").getByRole("button", { name: "Actions for legacy-threshold" }).click()
    await page.getByRole("menuitem", { name: "Edit" }).click()
    editor = page.getByRole("dialog", { name: "Edit legacy-threshold" })
    await expect(editor.getByText("External flags cannot be deleted here.")).toBeVisible()
    await expect(editor.getByText(/Consumers receive defaults/)).toHaveCount(0)
  })

})

async function mountFeatureFlagsHarness(page: import("@playwright/test").Page): Promise<void> {
  await page.goto("/e2e/fixtures/feature-flags-harness.html")
}

test.describe("feature flag refresh", () => {
  test("keeps the forced config result when an older request resolves later", async ({ page }) => {
    await mockAuth(page)

    let resolveInitial: (() => void) | undefined
    const initialReady = new Promise<void>(resolve => {
      resolveInitial = resolve
    })
    let configRequests = 0
    await page.route("**/api/config", async route => {
      configRequests += 1
      if (configRequests === 1) {
        await initialReady
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify({ admin: true, billing: false }),
        })
        return
      }
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ admin: false, billing: false }),
      })
    })

    await mountFeatureFlagsHarness(page)
    await expect(page.getByTestId("flags-state")).toHaveText("false:false")
    await page.getByRole("button", { name: "Refresh config" }).click()
    await expect.poll(() => configRequests).toBe(2)
    await expect(page.getByTestId("flags-state")).toHaveText("false:true")

    resolveInitial?.()
    await expect(page.getByTestId("flags-state")).toHaveText("false:true")
    await page.getByRole("button", { name: "Read cached config" }).click()
    await expect(page.getByTestId("cached-state")).toHaveText("false:false")
  })

  test("rejects failed refreshes while failing closed in context", async ({ page }) => {
    await mockAuth(page)

    let configRequests = 0
    await page.route("**/api/config", async route => {
      configRequests += 1
      if (configRequests === 1) {
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify({ admin: true, billing: false }),
        })
        return
      }
      await route.abort("failed")
    })

    await mountFeatureFlagsHarness(page)
    await expect(page.getByTestId("flags-state")).toHaveText("true:true")
    await page.getByRole("button", { name: "Refresh config" }).click()
    await expect(page.getByTestId("refresh-outcome")).toHaveText("rejected")
    await expect(page.getByTestId("flags-state")).toHaveText("false:true")
  })

  test("hides stale admin navigation and route content while refresh resolves", async ({ page }) => {
    await mockAuth(page)

    let resolveRefresh: (() => void) | undefined
    const refreshReady = new Promise<void>(resolve => {
      resolveRefresh = resolve
    })
    let configRequests = 0
    await page.route("**/api/config", async route => {
      configRequests += 1
      if (configRequests === 1) {
        await route.fulfill({
          contentType: "application/json",
          body: JSON.stringify({ admin: true, billing: false }),
        })
        return
      }
      await refreshReady
      await route.fulfill({
        contentType: "application/json",
        body: JSON.stringify({ admin: false, billing: false }),
      })
    })

    await mountFeatureFlagsHarness(page)
    await expect(page.getByRole("link", { name: "Feature flags" })).toBeVisible()
    await expect(page.getByRole("heading", { name: "Feature flags" })).toBeVisible()

    await page.getByRole("button", { name: "Refresh config" }).click()
    await expect.poll(() => configRequests).toBe(2)
    await expect(page.getByTestId("flags-state")).toHaveText("false:false")
    await expect(page.getByRole("link", { name: "Feature flags" })).toHaveCount(0)
    await expect(page.getByRole("heading", { name: "Feature flags" })).toHaveCount(0)

    resolveRefresh?.()
    await expect(page.getByTestId("flags-state")).toHaveText("false:true")
  })
})
