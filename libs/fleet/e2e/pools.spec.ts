import { test, expect } from "@playwright/test"
import {
  mockAuth,
  mockInstanceDetailApi,
  mockInstancesApi,
  mockNamespacesApi,
  mockPoolsApi,
  mockClaimsApi,
} from "./fixtures/mock-api"

test.describe("Pools page header", () => {
  for (const viewport of [
    { name: "desktop", width: 1440, height: 900 },
    { name: "desktop breakpoint", width: 1024, height: 900 },
    { name: "tablet", width: 768, height: 900 },
    { name: "narrow", width: 500, height: 900 },
    { name: "mobile", width: 375, height: 900 },
    { name: "compact", width: 320, height: 900 },
  ]) {
    test(`header stays stable without the shader on ${viewport.name}`, async ({
      page,
    }) => {
      await page.setViewportSize(viewport)
      await mockAuth(page)
      await mockNamespacesApi(page)
      await mockPoolsApi(page)

      await page.goto("/pools")

      const mesh = page.locator(".cua-pagehead__mesh")
      const topNavigation = page.locator("#cua-shell-topnav")
      const header = page.locator(".cua-pagehead")
      const semanticMain = page.getByRole("main")
      const navigation = page.getByRole("navigation", { name: "Main navigation" })
      const table = page.locator(".cua-pagebody")
      const title = page.locator(".cua-pagehead__title")
      const refreshPools = page.getByRole("button", { name: "Refresh pools" })
      const newPool = page.getByRole("button", { name: "New pool" })
      const status = page.getByRole("columnheader", { name: "Status" })

      await expect(mesh).toHaveCount(0)
      await expect(page).toHaveTitle("Pools · Cua")
      await expect(page.locator(".cua-pagehead canvas")).toHaveCount(0)
      const [topNavigationBox, navigationBox, headerBox, tableBox] = await Promise.all([
        topNavigation.boundingBox(),
        navigation.boundingBox(),
        header.boundingBox(),
        table.boundingBox(),
      ])
      const navigationOpen = await page
        .getByRole("button", { name: "Close navigation" })
        .isVisible()
      expect(topNavigationBox).not.toBeNull()
      expect(topNavigationBox!.x).toBeLessThanOrEqual(1)
      expect(Math.abs(topNavigationBox!.width - viewport.width)).toBeLessThanOrEqual(1)
      expect(headerBox).not.toBeNull()
      expect(tableBox).not.toBeNull()

      const paneStart = navigationOpen && navigationBox
        ? navigationBox.x + navigationBox.width
        : 0
      expect(headerBox!.x - paneStart).toBeGreaterThan(1)
      expect(viewport.width - (headerBox!.x + headerBox!.width)).toBeGreaterThan(1)
      expect(Math.abs(headerBox!.x - tableBox!.x)).toBeLessThanOrEqual(1)
      expect(
        Math.abs(
          headerBox!.x + headerBox!.width - (tableBox!.x + tableBox!.width),
        ),
      ).toBeLessThanOrEqual(1)
      expect(
        await semanticMain.evaluate(
          element => element.scrollWidth - element.clientWidth,
        ),
      ).toBeLessThanOrEqual(1)
      expect(
        await page.locator("html").evaluate(
          element => element.scrollWidth - element.clientWidth,
        ),
      ).toBeLessThanOrEqual(1)
      await expect(page.getByRole("button", { name: "testuser" })).toBeVisible()

      const geometrySamples = await header.evaluate(async element => {
        const samples: string[] = []
        for (let frame = 0; frame < 30; frame += 1) {
          await new Promise<void>(resolve => requestAnimationFrame(() => resolve()))
          const box = element.getBoundingClientRect()
          samples.push(
            [box.x, box.width, document.documentElement.scrollWidth].join(":"),
          )
        }
        return samples
      })
      expect(new Set(geometrySamples).size).toBe(1)

      await expect(title).toHaveCSS(
        "font-family",
        /Urbanist/,
      )
      await expect(title).toHaveCSS(
        "font-size",
        viewport.width >= 688 ? "28px" : "24px",
      )
      await expect(status).toBeVisible()
      await expect(newPool).toHaveCSS("border-radius", "10px")
      await expect(newPool).toHaveCSS("font-size", "13px")
      await expect(newPool).toHaveCSS("appearance", "none")
      await expect(newPool).toHaveCSS("box-shadow", "none")
      expect(await newPool.evaluate(element => getComputedStyle(element).backgroundImage))
        .toContain("linear-gradient")
      await page.evaluate(() => {
        document.body.setAttribute("data-awsui-focus-visible", "true")
      })
      await newPool.focus()
      expect(
        await newPool.evaluate(
          element => getComputedStyle(element, "::before").content,
        ),
      ).toBe("none")
      await expect(newPool).toHaveCSS("outline-style", "solid")
      await expect(newPool).toHaveCSS("outline-width", "2px")
      await expect(newPool).toHaveCSS("outline-offset", "2px")
      await newPool.hover()
      expect(await newPool.evaluate(element => getComputedStyle(element).backgroundImage))
        .toContain("linear-gradient")
      await refreshPools.hover()
      await expect(refreshPools).toHaveCSS("background-color", "rgb(32, 33, 36)")

      const [refreshBox, refreshIconBox] = await Promise.all([
        refreshPools.boundingBox(),
        refreshPools.locator("svg").boundingBox(),
      ])
      expect(refreshBox).not.toBeNull()
      expect(refreshIconBox).not.toBeNull()
      expect(
        Math.abs(
          refreshBox!.x + refreshBox!.width / 2 -
            (refreshIconBox!.x + refreshIconBox!.width / 2),
        ),
      ).toBeLessThanOrEqual(0.5)
      expect(
        Math.abs(
          refreshBox!.y + refreshBox!.height / 2 -
            (refreshIconBox!.y + refreshIconBox!.height / 2),
        ),
      ).toBeLessThanOrEqual(0.5)

      expect(navigationOpen).toBe(viewport.width >= 1024)

      if (viewport.width < 1024) {
        const openNavigation = page.getByRole("button", { name: "Open navigation" })
        await expect(openNavigation).toHaveCSS("border-radius", "10px")
        await expect(openNavigation).toHaveCSS("width", "40px")
        await expect(openNavigation).toHaveCSS("height", "40px")
        await openNavigation.click()
        const openNavigationBox = await page
          .getByRole("navigation", { name: "Main navigation" })
          .boundingBox()
        expect(openNavigationBox).not.toBeNull()
        expect(openNavigationBox!.y).toBeLessThanOrEqual(
          viewport.width < 688 ? 1 : 50,
        )
      }
    })
  }
})

test("Pools remains horizontally usable at 200% zoom and 320 CSS pixels", async ({
  page,
}) => {
  await page.setViewportSize({ width: 640, height: 900 })
  await mockAuth(page)
  await mockNamespacesApi(page)
  await mockPoolsApi(page)
  await page.goto("/pools")

  // CSS zoom exercises the same two-dimensional reflow pressure as 200%
  // browser zoom while remaining deterministic in headless Chromium.
  await page.locator("html").evaluate(element => {
    element.style.zoom = "200%"
  })
  await expect(page.locator(".cua-pagebody")).toBeVisible()
  expect(
    await page.locator("html").evaluate(
      element => element.scrollWidth - element.clientWidth,
    ),
  ).toBeLessThanOrEqual(1)
  expect(
    await page.getByRole("main").evaluate(
      element => element.scrollWidth - element.clientWidth,
    ),
  ).toBeLessThanOrEqual(1)

  await page.locator("html").evaluate(element => {
    element.style.zoom = ""
  })
  await page.setViewportSize({ width: 320, height: 900 })
  await expect(page.getByRole("button", { name: "New pool" })).toBeVisible()
  expect(
    await page.locator("html").evaluate(
      element => element.scrollWidth - element.clientWidth,
    ),
  ).toBeLessThanOrEqual(1)
})

test("forced colors removes decoration and navigation works from the keyboard", async ({
  page,
}) => {
  await page.setViewportSize({ width: 500, height: 900 })
  await page.emulateMedia({ forcedColors: "active" })
  await mockAuth(page)
  await mockNamespacesApi(page)
  await mockPoolsApi(page)
  await page.goto("/pools")

  const topNavigation = page.locator(".cua-shell__topnav")
  const pageHead = page.locator(".cua-pagehead")
  expect(
    await topNavigation.evaluate(
      element => getComputedStyle(element, "::before").display,
    ),
  ).toBe("none")
  expect(
    await pageHead.evaluate(
      element => getComputedStyle(element, "::after").display,
    ),
  ).toBe("none")

  const openNavigation = page.getByRole("button", { name: "Open navigation" })
  await openNavigation.focus()
  await openNavigation.press("Enter")
  const closeNavigation = page.getByRole("button", {
    name: "Close navigation",
  })
  await expect(closeNavigation).toBeVisible()
  await closeNavigation.focus()
  await closeNavigation.press("Space")
  await expect(openNavigation).toBeVisible()

  expect(
    await page.locator("main").evaluate(
      element => element.scrollWidth - element.clientWidth,
    ),
  ).toBeLessThanOrEqual(1)
})

test("selection actions stay with the table and deletion requires confirmation", async ({
  page,
}) => {
  await mockAuth(page)
  await mockNamespacesApi(page)
  await mockPoolsApi(page)
  await page.goto("/pools")

  await expect(page.getByRole("button", { name: "Duplicate" })).toHaveCount(0)
  await expect(page.getByRole("button", { name: "Delete" })).toHaveCount(0)

  const row = page.getByRole("row").filter({ hasText: "demo-pool" })
  await row.getByRole("checkbox").check()
  await expect(page.getByText("1 pool selected")).toBeVisible()
  await page.getByRole("button", { name: "Delete" }).click()

  const dialog = page.getByRole("dialog", { name: "Delete 1 pool?" })
  await expect(dialog).toBeVisible()
  await expect(dialog.getByText("This action cannot be undone.")).toBeVisible()
  await dialog.getByRole("button", { name: "Cancel" }).click()
  await expect(dialog).toBeHidden()
})

test("local visual preview keeps preview data active on pool detail", async ({
  page,
}) => {
  await page.goto("/pools?cua-visual-preview")

  await page.getByRole("link", { name: "prod-web-fleet" }).click()

  await expect(page).toHaveURL(
    /\/pools\/preview\/prod-web-fleet\?cua-visual-preview$/,
  )
  await expect(
    page.getByRole("heading", { name: "prod-web-fleet" }),
  ).toBeVisible()
  await expect(page.getByText("Failed to load pool")).toHaveCount(0)
  await expect(page.getByRole("heading", { name: "Instances" })).toBeVisible()
})

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

  test("invalid submission focuses the name and cancel protects unsaved work", async ({
    page,
  }) => {
    await mockAuth(page)
    await mockNamespacesApi(page)
    await mockPoolsApi(page)
    await page.goto("/pools/new")

    const name = page.getByRole("textbox", { name: "Name" })
    await page.getByRole("button", { name: "Create" }).click()
    await expect(name).toBeFocused()
    await expect(page.getByText("Name is required.")).toBeVisible()

    await name.fill("new-fleet")
    await page.getByRole("button", { name: "Cancel" }).click()
    const dialog = page.getByRole("dialog", { name: "Discard changes?" })
    await expect(dialog).toBeVisible()
    await dialog.getByRole("button", { name: "Keep editing" }).click()
    await expect(name).toHaveValue("new-fleet")
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

test.describe("Pool detail instances", () => {
  test("shows warm pool instances and claim state", async ({ page }) => {
    await mockAuth(page)
    await mockNamespacesApi(page)
    await mockPoolsApi(page)
    await mockInstancesApi(page)
    await mockClaimsApi(page)

    await page.goto("/pools/demo-pool/demo-pool")

    // The pool name should be shown
    await expect(page.getByRole("heading", { name: "demo-pool" })).toBeVisible()

    await expect(page.getByRole("heading", { name: "Instances" })).toBeVisible()

    // The "Create claim" button should be visible
    await expect(
      page.getByRole("button", { name: "Create claim" }),
    ).toBeVisible()
    await expect(page.getByRole("button", { name: "Create claim" })).toHaveCSS(
      "border-radius",
      "10px",
    )
    await expect(page.getByRole("button", { name: "Refresh instances" })).toHaveCSS(
      "border-radius",
      "10px",
    )
    await expect(page.getByRole("button", { name: "Refresh instances" })).toHaveCSS(
      "width",
      "40px",
    )

    const claimedRow = page.getByRole("row").filter({ hasText: "vm-xyz789" })
    await expect(claimedRow).toContainText("Ready")
    await expect(claimedRow).toContainText("claim-abc123")
    await expect(claimedRow).toContainText("Bound")
    await expect(claimedRow.getByRole("button", { name: "Release" })).toBeVisible()

    const availableRow = page.getByRole("row").filter({ hasText: "vm-ready456" })
    await expect(availableRow).toContainText("Ready")
    await expect(availableRow).toContainText("Unclaimed")
    await expect(availableRow.getByRole("button", { name: "Release" })).toHaveCount(0)
    await expect(page.getByText("other-pool-instance")).toHaveCount(0)
  })

  test("opens instance details and displays guest console logs", async ({ page }) => {
    await mockAuth(page)
    await mockNamespacesApi(page)
    await mockPoolsApi(page)
    await mockInstancesApi(page)
    await mockClaimsApi(page)
    await mockInstanceDetailApi(page)
    await page.context().grantPermissions(
      ["clipboard-read", "clipboard-write"],
      { origin: "http://localhost:5180" },
    )

    await page.goto("/pools/demo-pool/demo-pool")
    await page.getByRole("link", { name: "vm-xyz789" }).click()

    await expect(page).toHaveURL(
      /\/pools\/demo-pool\/demo-pool\/instances\/vm-xyz789$/,
    )
    await expect(page.getByRole("heading", { name: "vm-xyz789" })).toBeVisible()
    await expect(
      page.getByRole("heading", { name: "Guest console logs", exact: true }),
    ).toBeVisible()
    await expect(page.getByRole("heading", { name: "Overview" })).toHaveCount(0)
    await expect(page.getByLabel("Guest console logs")).toContainText(
      "guest console ready",
    )
    await expect(
      page.getByRole("button", { name: "Scroll logs to top" }),
    ).toBeEnabled()
    await expect(
      page.getByRole("button", { name: "Scroll logs to bottom" }),
    ).toBeEnabled()
    await page.getByRole("button", { name: "Scroll logs to bottom" }).click()
    await page.getByRole("button", { name: "Scroll logs to top" }).click()
    await page.getByRole("button", { name: "Copy logs" }).click()
    await expect(page.getByRole("button", { name: "Copied" })).toBeVisible()
    await expect
      .poll(() => page.evaluate(() => navigator.clipboard.readText()))
      .toContain("guest console ready")
    await expect(page.getByLabel("Log container")).toHaveCount(0)
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
