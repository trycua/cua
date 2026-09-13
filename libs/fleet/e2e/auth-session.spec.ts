import { expect, test } from "@playwright/test"
import {
  mockAuth,
  mockGitHubTrustPoliciesApi,
  mockNamespacesApi,
} from "./fixtures/mock-api"

interface MockKeycloakState {
  initOptions: Record<string, unknown> | null
  loginOptions: Record<string, unknown> | null
  logoutOptions: Record<string, unknown> | null
  updateTokenMinValidity: number[]
  clearTokenCalls: number
}

function keycloakState(): MockKeycloakState {
  return (window as unknown as { __MOCK_KEYCLOAK__: MockKeycloakState })
    .__MOCK_KEYCLOAK__
}

test.beforeEach(async ({ page }) => {
  await mockNamespacesApi(page)
  await mockGitHubTrustPoliciesApi(page)
})

test("gates the protected shell until standard PKCE authentication resolves", async ({
  page,
}) => {
  const auth = await mockAuth(page, {}, { holdAuth: true })

  await page.goto("/settings")
  const bootSurface = page.locator("#cua-boot")
  await expect(bootSurface).toBeVisible()
  await expect(bootSurface.getByText("Signing you in")).toBeVisible()
  await expect(bootSurface.getByText("Continuing to auth.cua.ai")).toBeVisible()
  await expect
    .poll(() =>
      bootSurface.evaluate(element => getComputedStyle(element).backgroundImage),
    )
    .toContain("radial-gradient")
  await expect(page.getByRole("navigation")).toHaveCount(0)

  const initOptions = await page.evaluate(keycloakState).then(state => state.initOptions)
  expect(initOptions).toMatchObject({
    onLoad: "login-required",
    flow: "standard",
    pkceMethod: "S256",
    checkLoginIframe: false,
  })

  await auth.releaseAuth()
  await expect(bootSurface).toHaveCount(0)
  await expect(page.getByRole("button", { name: "testuser" })).toBeVisible()
})

test("refreshes API tokens with a thirty-second safety window", async ({ page }) => {
  await mockAuth(page)

  await page.goto("/settings")
  await expect(page.getByRole("button", { name: "testuser" })).toBeVisible()

  await expect
    .poll(() =>
      page.evaluate(keycloakState).then(state => state.updateTokenMinValidity),
    )
    .toContain(30)
})

test("failed refresh restarts login without replaying callback artifacts", async ({
  page,
}) => {
  await mockAuth(page, {}, { refreshFails: true })

  await page.goto(
    "/settings?tab=billing&code=one-time&state=opaque&session_state=opaque",
  )

  await expect
    .poll(() => page.evaluate(keycloakState).then(state => state.loginOptions))
    .toEqual({ redirectUri: "http://localhost:5180/settings?tab=billing" })
  await expect
    .poll(() => page.evaluate(keycloakState).then(state => state.clearTokenCalls))
    .toBe(1)
})

test("rejected callback fails closed without rendering the shell", async ({ page }) => {
  await mockAuth(page, {}, { initRejects: true })

  await page.goto("/settings?error=access_denied&state=opaque")

  await expect(
    page.getByRole("heading", { name: "We couldn't sign you in" }),
  ).toBeVisible()
  await expect(page.getByText("Error: rejected callback")).toBeVisible()
  await expect(page.getByRole("button", { name: "Try again" })).toBeVisible()
  await expect(page.locator("#cua-boot")).toHaveCount(0)
  await expect(page.getByRole("button", { name: "testuser" })).toHaveCount(0)
})

test("logout uses the stable same-origin Run entry point", async ({ page }) => {
  await mockAuth(page)

  await page.goto("/settings?tab=billing")
  await page.getByRole("button", { name: "testuser" }).click()
  await page.getByRole("menuitem", { name: "Sign out" }).press("Enter")

  await expect
    .poll(() => page.evaluate(keycloakState).then(state => state.logoutOptions))
    .toEqual({ redirectUri: "http://localhost:5180/" })
})

test("account menu opens without a redundant identity header", async ({ page }) => {
  await mockAuth(page)

  await page.goto("/settings")
  await page.getByRole("button", { name: "testuser" }).click()

  const topNavigation = page.locator("#cua-shell-topnav")
  const signOut = page.getByRole("menuitem", { name: "Sign out" })

  await expect(signOut).toBeVisible()
  await expect(
    page.locator('[id^="awsui-button-dropdown__header"]:visible'),
  ).toHaveCount(0)

  const [topNavigationBox, signOutBox] = await Promise.all([
    topNavigation.boundingBox(),
    signOut.boundingBox(),
  ])
  expect(topNavigationBox).not.toBeNull()
  expect(signOutBox).not.toBeNull()
  expect(signOutBox!.y).toBeGreaterThanOrEqual(
    topNavigationBox!.y + topNavigationBox!.height,
  )
  expect(
    await signOut.evaluate(element => {
      const box = element.getBoundingClientRect()
      const topmost = document.elementFromPoint(
        box.left + box.width / 2,
        box.top + box.height / 2,
      )
      return topmost === element || element.contains(topmost)
    }),
  ).toBe(true)
})
