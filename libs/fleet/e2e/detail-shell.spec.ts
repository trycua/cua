import { expect, test } from "@playwright/test"
import {
  mockAuth,
  mockClaimsApi,
  mockInstancesApi,
  mockNamespacesApi,
  mockPoolsApi,
  mockSignedServiceUrlsApi,
} from "./fixtures/mock-api"
import { expectSharedPageShell } from "./fixtures/shell-geometry"

async function mockFleet(
  page: Parameters<typeof mockAuth>[0],
  initialSignedUrls: Parameters<typeof mockSignedServiceUrlsApi>[1] = [],
  signedUrlOptions: Parameters<typeof mockSignedServiceUrlsApi>[2] = {},
  claimOptions: Parameters<typeof mockClaimsApi>[1] = {},
) {
  await mockAuth(page)
  await mockNamespacesApi(page)
  await mockPoolsApi(page)
  await mockInstancesApi(page)
  const claims = await mockClaimsApi(page, claimOptions)
  const signedUrls = await mockSignedServiceUrlsApi(page, initialSignedUrls, signedUrlOptions)
  return { ...signedUrls, claims }
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

test("claim detail manages multiple signed URLs through the shared SDK", async ({ page }) => {
  const signedUrls = await mockFleet(page, [
    {
      id: "11111111-1111-4111-8111-111111111111",
      namespace: "demo-pool",
      claim: "claim-abc123",
      sandbox: "vm-xyz789",
      service: "vm-xyz789-mcp",
      logicalService: "mcp",
      label: "Customer demo",
      url: "https://run.cua.ai/api/signed/11111111-1111-4111-8111-111111111111",
      createdAt: "2099-01-01T00:00:00Z",
      expiresAt: "2099-01-01T01:00:00Z",
    },
    {
      id: "22222222-2222-4222-8222-222222222222",
      namespace: "demo-pool",
      claim: "claim-abc123",
      sandbox: "vm-xyz789",
      service: "vm-xyz789-mcp",
      logicalService: "mcp",
      label: "QA session",
      url: "https://run.cua.ai/api/signed/22222222-2222-4222-8222-222222222222",
      createdAt: "2099-01-01T00:00:00Z",
      expiresAt: "2099-01-01T01:00:00Z",
    },
  ])
  await page.goto("/pools/demo-pool/demo-pool/claims/claim-abc123")

  await expect(page.getByRole("heading", { name: "Signed URLs (2)" })).toBeVisible()
  await expect(page.getByRole("cell", { name: "Customer demo" })).toBeVisible()
  await expect(page.getByRole("cell", { name: "QA session" })).toBeVisible()
  await expect(page.getByRole("link", { name: "/api/svc/demo-pool/vm-xyz789-mcp/" })).toBeVisible()

  await page.getByRole("button", { name: "Create signed URL" }).click()
  await page.getByRole("button", { name: "Choose a service" }).click()
  await page.getByRole("option", { name: "mcp" }).click()
  await page.getByLabel("Label (optional)").fill("Partner preview")
  await page.getByLabel("1 hour").check()
  await page.getByRole("button", { name: "Create signed URL" }).first().click()

  await expect.poll(() => signedUrls.requests.findLast(request => request.method === "POST")?.body).toMatchObject({
    logicalService: "mcp",
    service: "vm-xyz789-mcp",
    expiresInSeconds: 3600,
    label: "Partner preview",
  })
  await expect(page.getByRole("cell", { name: "Partner preview" })).toBeVisible()
  expect(signedUrls.urls.find((item) => item.label === "Partner preview")?.expiresAt).toBe("2099-01-01T01:00:00.000Z")
  await expect(page.getByRole("button", { name: "Copy signed URL 11111111-1111-4111-8111-111111111111" })).toBeVisible()
  await expect(page.getByRole("button", { name: "Copy signed URL 22222222-2222-4222-8222-222222222222" })).toBeVisible()

  await page.getByRole("button", { name: "Create signed URL" }).click()
  await page.getByRole("button", { name: "Choose a service" }).click()
  await page.getByRole("option", { name: "mcp" }).click()
  await page.getByLabel("24 hours").check()
  await page.getByRole("button", { name: "Create signed URL" }).first().click()
  await expect.poll(() => signedUrls.urls.at(-1)?.expiresAt).toBe("2099-01-02T00:00:00.000Z")

  await page.getByRole("button", { name: "Revoke signed URL 11111111-1111-4111-8111-111111111111" }).click()
  await expect.poll(() => signedUrls.requests.findLast(request => request.method === "DELETE")).toMatchObject({
    method: "DELETE",
    id: "11111111-1111-4111-8111-111111111111",
  })
  await expect(page.getByText("Revoked", { exact: true })).toBeVisible()
  await expect(page.getByRole("cell", { name: "QA session" })).toBeVisible()
})

test("claim detail renders signed URL lifecycle and empty states", async ({ page }) => {
  await mockFleet(page, [
    {
      id: "33333333-3333-4333-8333-333333333333",
      namespace: "demo-pool",
      claim: "claim-abc123",
      sandbox: "vm-xyz789",
      service: "vm-xyz789-mcp",
      logicalService: "mcp",
      url: "https://run.cua.ai/api/signed/33333333-3333-4333-8333-333333333333",
      createdAt: "2026-08-30T12:00:00Z",
      expiresAt: "2026-08-30T13:00:00Z",
    },
    {
      id: "44444444-4444-4444-8444-444444444444",
      namespace: "demo-pool",
      claim: "claim-abc123",
      sandbox: "vm-xyz789",
      service: "vm-xyz789-mcp",
      logicalService: "mcp",
      url: "https://run.cua.ai/api/signed/44444444-4444-4444-8444-444444444444",
      createdAt: "2099-01-01T00:00:00Z",
      expiresAt: "2099-01-02T00:00:00Z",
      revokedAt: "2099-01-01T00:30:00Z",
    },
  ])
  await page.goto("/pools/demo-pool/demo-pool/claims/claim-abc123")

  await expect(page.getByText("Expired", { exact: true })).toBeVisible()
  await expect(page.getByText("Revoked", { exact: true })).toBeVisible()

  await page.getByRole("button", { name: "Refresh signed URLs" }).click()
})

test("claim detail offers an actionable signed URL empty state", async ({ page }) => {
  await mockFleet(page)
  await page.goto("/pools/demo-pool/demo-pool/claims/claim-abc123")

  await expect(page.getByText("No signed URLs yet.")).toBeVisible()
  await page.getByRole("button", { name: "Create signed URL" }).first().click()
  await expect(page.getByText("Anyone with this bearer link can access the selected service until it expires or is revoked.")).toBeVisible()
})


test("claim detail ignores a late signed URL list after sandbox navigation", async ({ page }) => {
  const signedUrls = await mockFleet(
    page,
    [
      {
        id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        namespace: "demo-pool",
        claim: "claim-abc123",
        sandbox: "vm-xyz789",
        service: "vm-xyz789-mcp",
        logicalService: "mcp",
        label: "Stale claim A URL",
        url: "https://run.cua.ai/api/signed/aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        createdAt: "2099-01-01T00:00:00Z",
        expiresAt: "2099-01-01T01:00:00Z",
      },
      {
        id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        namespace: "demo-pool",
        claim: "claim-next",
        sandbox: "vm-next",
        service: "vm-next-mcp",
        logicalService: "mcp",
        label: "Current claim B URL",
        url: "https://run.cua.ai/api/signed/bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb",
        createdAt: "2099-01-01T00:00:00Z",
        expiresAt: "2099-01-01T01:00:00Z",
      },
    ],
    { holdListClaims: ["claim-abc123"] },
    {
      claims: [
        { name: "claim-abc123", sandboxName: "vm-xyz789", sandboxService: "vm-xyz789-svc" },
        { name: "claim-next", sandboxName: "vm-next", sandboxService: "vm-next-svc" },
      ],
    },
  )
  await page.goto("/pools/demo-pool/demo-pool/claims/claim-abc123")
  await expect.poll(() => signedUrls.requests.filter((request) => request.method === "GET").length).toBeGreaterThanOrEqual(1)

  signedUrls.claims.holdNextLists(2)
  await page.evaluate(() => {
    window.history.pushState({}, "", "/pools/demo-pool/demo-pool/claims/claim-next")
    window.dispatchEvent(new PopStateEvent("popstate"))
  })
  await expect(page.getByText("Loading claim…", { exact: true })).toBeVisible()

  signedUrls.releaseList("claim-abc123")
  await page.waitForTimeout(100)
  await expect(page.getByText("Loading claim…", { exact: true })).toBeVisible()
  await expect(page.getByRole("cell", { name: "Stale claim A URL" })).toHaveCount(0)
  await expect(page.getByText("Signed URLs are unavailable for this deployment. Other claim details remain available.")).toHaveCount(0)

  signedUrls.claims.releaseLists()
  await expect(page.getByRole("heading", { name: "claim-next", level: 1 })).toBeVisible()
  await expect(page.getByRole("cell", { name: "Current claim B URL" })).toBeVisible()
  await expect(page.getByRole("cell", { name: "Stale claim A URL" })).toHaveCount(0)
})

test("claim detail hides signed URL controls when creation is unavailable", async ({ page }) => {
  await mockFleet(page, [], { unavailable: { create: true } })
  await page.goto("/pools/demo-pool/demo-pool/claims/claim-abc123")

  await page.getByRole("button", { name: "Create signed URL" }).click()
  await page.getByRole("button", { name: "Choose a service" }).click()
  await page.getByRole("option", { name: "mcp" }).click()
  await page.getByRole("button", { name: "Create signed URL" }).first().click()

  await expect(page.getByText("Signed URLs are unavailable for this deployment. Other claim details remain available.")).toBeVisible()
  await expect(page.getByRole("button", { name: "Create signed URL" })).toHaveCount(0)
  await expect(page.getByRole("button", { name: "Refresh signed URLs" })).toHaveCount(0)
  await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible()
})

test("claim detail hides signed URL controls when revocation is unavailable", async ({ page }) => {
  await mockFleet(page, [
    {
      id: "cccccccc-cccc-4ccc-8ccc-cccccccccccc",
      namespace: "demo-pool",
      claim: "claim-abc123",
      sandbox: "vm-xyz789",
      service: "vm-xyz789-mcp",
      logicalService: "mcp",
      label: "Revoke unavailable",
      url: "https://run.cua.ai/api/signed/cccccccc-cccc-4ccc-8ccc-cccccccccccc",
      createdAt: "2099-01-01T00:00:00Z",
      expiresAt: "2099-01-01T01:00:00Z",
    },
  ], { unavailable: { revoke: true } })
  await page.goto("/pools/demo-pool/demo-pool/claims/claim-abc123")

  await page.getByRole("button", { name: "Revoke signed URL cccccccc-cccc-4ccc-8ccc-cccccccccccc" }).click()

  await expect(page.getByText("Signed URLs are unavailable for this deployment. Other claim details remain available.")).toBeVisible()
  await expect(page.getByRole("button", { name: "Revoke signed URL cccccccc-cccc-4ccc-8ccc-cccccccccccc" })).toHaveCount(0)
  await expect(page.getByRole("heading", { name: "Overview" })).toBeVisible()
})

test("mobile claim detail creates and revokes a signed URL", async ({ page }) => {
  await page.setViewportSize({ width: 375, height: 900 })
  const signedUrls = await mockFleet(page)
  await page.goto("/pools/demo-pool/demo-pool/claims/claim-abc123")

  await page.getByRole("button", { name: "Create signed URL" }).first().click()
  await page.getByRole("button", { name: "Choose a service" }).click()
  await page.getByRole("option", { name: "mcp" }).click()
  await page.getByLabel("15 minutes").check()
  await page.getByRole("button", { name: "Create signed URL" }).first().click()

  await expect.poll(() => signedUrls.urls[0]?.expiresAt).toBe("2099-01-01T00:15:00.000Z")
  const created = signedUrls.urls[0]
  await expect(page.getByRole("button", { name: `Revoke signed URL ${created.id}` })).toBeVisible()
  await page.getByRole("button", { name: `Revoke signed URL ${created.id}` }).click()
  await expect(page.getByText("Revoked", { exact: true })).toBeVisible()
})

test("claim navigation ignores a delayed signed URL creation from the prior sandbox identity", async ({ page }) => {
  const signedUrls = await mockFleet(
    page,
    [
      {
        id: "dddddddd-dddd-4ddd-8ddd-dddddddddddd",
        namespace: "demo-pool",
        claim: "claim-next",
        sandbox: "vm-xyz789",
        service: "vm-xyz789-mcp",
        logicalService: "mcp",
        label: "Current claim URL",
        url: "https://run.cua.ai/api/signed/dddddddd-dddd-4ddd-8ddd-dddddddddddd",
        createdAt: "2099-01-01T00:00:00Z",
        expiresAt: "2099-01-01T01:00:00Z",
      },
    ],
    { holdCreateClaims: ["claim-abc123"] },
    {
      claims: [
        { name: "claim-abc123", sandboxName: "vm-xyz789", sandboxService: "vm-xyz789-svc" },
        { name: "claim-next", sandboxName: "vm-xyz789", sandboxService: "vm-xyz789-svc" },
      ],
    },
  )
  await page.goto("/pools/demo-pool/demo-pool/claims/claim-abc123")

  await page.getByRole("button", { name: "Create signed URL" }).first().click()
  await page.getByRole("button", { name: "Choose a service" }).click()
  await page.getByRole("option", { name: "mcp" }).click()
  await page.getByLabel("Label (optional)").fill("Prior claim bearer URL")
  await page.getByRole("button", { name: "Create signed URL" }).first().click()
  await expect.poll(() => signedUrls.requests.some((request) => request.method === "POST")).toBe(true)

  signedUrls.claims.holdNextLists(2)
  await page.evaluate(() => {
    window.history.pushState({}, "", "/pools/demo-pool/demo-pool/claims/claim-next")
    window.dispatchEvent(new PopStateEvent("popstate"))
  })
  await expect(page.getByText("Loading claim…", { exact: true })).toBeVisible()

  signedUrls.releaseCreate("claim-abc123")
  await expect.poll(() => signedUrls.urls.some((item) => item.label === "Prior claim bearer URL")).toBe(true)
  await page.waitForTimeout(100)
  await expect(page.getByText("Loading claim…", { exact: true })).toBeVisible()
  await expect(page.getByRole("cell", { name: "Prior claim bearer URL" })).toHaveCount(0)
  await expect(page.getByText("Signed URL created", { exact: true })).toHaveCount(0)
  await expect(page.getByText("Signed URLs are unavailable for this deployment. Other claim details remain available.")).toHaveCount(0)

  signedUrls.claims.releaseLists()
  await expect(page.getByRole("heading", { name: "claim-next", level: 1 })).toBeVisible()
  await expect(page.getByRole("cell", { name: "Current claim URL" })).toBeVisible()
  await expect(page.getByRole("cell", { name: "Prior claim bearer URL" })).toHaveCount(0)
})

test("claim navigation ignores delayed signed URL revocation unavailability from the prior sandbox identity", async ({ page }) => {
  const oldUrlId = "eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee"
  const signedUrls = await mockFleet(
    page,
    [
      {
        id: oldUrlId,
        namespace: "demo-pool",
        claim: "claim-abc123",
        sandbox: "vm-xyz789",
        service: "vm-xyz789-mcp",
        logicalService: "mcp",
        label: "Prior claim URL",
        url: `https://run.cua.ai/api/signed/${oldUrlId}`,
        createdAt: "2099-01-01T00:00:00Z",
        expiresAt: "2099-01-01T01:00:00Z",
      },
      {
        id: "ffffffff-ffff-4fff-8fff-ffffffffffff",
        namespace: "demo-pool",
        claim: "claim-next",
        sandbox: "vm-xyz789",
        service: "vm-xyz789-mcp",
        logicalService: "mcp",
        label: "Current claim URL",
        url: "https://run.cua.ai/api/signed/ffffffff-ffff-4fff-8fff-ffffffffffff",
        createdAt: "2099-01-01T00:00:00Z",
        expiresAt: "2099-01-01T01:00:00Z",
      },
    ],
    { holdRevokeIds: [oldUrlId], unavailable: { revoke: true } },
    {
      claims: [
        { name: "claim-abc123", sandboxName: "vm-xyz789", sandboxService: "vm-xyz789-svc" },
        { name: "claim-next", sandboxName: "vm-xyz789", sandboxService: "vm-xyz789-svc" },
      ],
    },
  )
  await page.goto("/pools/demo-pool/demo-pool/claims/claim-abc123")
  await page.getByRole("button", { name: `Revoke signed URL ${oldUrlId}` }).click()
  await expect.poll(() => signedUrls.requests.some((request) => request.method === "DELETE" && request.id === oldUrlId)).toBe(true)

  signedUrls.claims.holdNextLists(2)
  await page.evaluate(() => {
    window.history.pushState({}, "", "/pools/demo-pool/demo-pool/claims/claim-next")
    window.dispatchEvent(new PopStateEvent("popstate"))
  })
  await expect(page.getByText("Loading claim…", { exact: true })).toBeVisible()

  signedUrls.releaseRevoke(oldUrlId)
  await page.waitForTimeout(100)
  await expect(page.getByText("Loading claim…", { exact: true })).toBeVisible()
  await expect(page.getByRole("cell", { name: "Prior claim URL" })).toHaveCount(0)
  await expect(page.getByText("Signed URLs are unavailable for this deployment. Other claim details remain available.")).toHaveCount(0)

  signedUrls.claims.releaseLists()
  await expect(page.getByRole("heading", { name: "claim-next", level: 1 })).toBeVisible()
  await expect(page.getByRole("cell", { name: "Current claim URL" })).toBeVisible()
  await expect(page.getByRole("cell", { name: "Prior claim URL" })).toHaveCount(0)
})
