import { expect, test, type Page } from "@playwright/test"
import { mockAuth, type MockFeatureFlags } from "./fixtures/mock-api"
import { expectSharedPageShell } from "./fixtures/shell-geometry"
const overview = {
  data_as_of: "2026-08-13T10:00:00Z",
  partial: true,
  pools: [
    {
      id: "ns-a:pool-a",
      name: "Pool A",
      cpu: { consumed: 12, provisioned: 24 },
      memory: { consumed: 48, provisioned: 96 },
    },
    {
      id: "ns-b:pool-b",
      name: "Pool B",
      cpu: { consumed: 6, provisioned: 12 },
      memory: { consumed: 20, provisioned: 40 },
    },
  ],
}
const detail = {
  data_as_of: "2026-08-13T10:00:00Z",
  partial: false,
  pool: { id: "ns-b:pool-b", name: "Pool B" },
  buckets: [
    {
      start: "2026-08-13T08:00:00Z",
      end: "2026-08-13T09:00:00Z",
      cpu_consumed: 0.5,
      cpu_provisioned: 1,
      memory_consumed: 2,
      memory_provisioned: 4,
    },
  ],
}
async function mockUsage(
  p: Page,
  flags: MockFeatureFlags = { admin: false, usage: true },
) {
  await mockAuth(p, flags)
  await p.route("**/api/usage/overview**", (r) => r.fulfill({ json: overview }))
  await p.route("**/api/usage/pool**", (r) => r.fulfill({ json: detail }))
}
test("usage navigation is feature flagged", async ({ page }) => {
  await mockUsage(page, { admin: false, usage: false })
  await page.goto("/pools")
  await expect(page.getByRole("link", { name: "Usage" })).toHaveCount(0)
  await page.goto("/usage")
  await expect(page).toHaveURL(/\/pools$/)
})
test("renders usage totals and forwards opaque pool IDs to drilldown", async ({
  page,
}) => {
  await mockUsage(page)
  await page.goto("/usage")
  await expect(page).toHaveTitle("Usage · Cua")
  await expectSharedPageShell(page)
  await expect(
    page.getByRole("heading", { name: "Usage preview" }),
  ).toBeVisible()
  await expect(
    page.getByText("Pool A: 12 consumed, 24 provisioned, 50% utilization"),
  ).toBeVisible()
  await expect(
    page.getByText(
      "Consumed values use measured usage; provisioned values use Kubernetes requests observed by OpenCost, grouped by pool for the selected timeframe.",
    ),
  ).toBeVisible()
  await page.getByRole("button", { name: "Memory" }).click()
  await page.getByLabel("Pool drilldown").click()
  const poolRequest = page.waitForRequest((request) => {
    const url = new URL(request.url())
    return (
      url.pathname === "/api/usage/pool" &&
      url.searchParams.get("pool") === "ns-b:pool-b"
    )
  })
  await page.getByRole("option", { name: "Pool B" }).click()
  expect((await poolRequest).url()).toContain("pool=ns-b%3Apool-b")
  await expect(
    page.getByRole("heading", { name: "CPU core-hours per bucket" }),
  ).toBeVisible()
  await expect(
    page.getByRole("heading", { name: "Memory GiB-hours per bucket" }),
  ).toBeVisible()
})
test("admin view-as persists", async ({ page }) => {
  await mockUsage(page, { admin: true, usage: true })
  await page.goto("/usage")
  await page.getByLabel("View usage for subject").fill("customer")
  await page.getByRole("button", { name: "View usage" }).click()
  await expect(page).toHaveURL(/subject=customer/)
  await expect(page.getByText("Viewing usage for customer")).toBeVisible()
})
test("shows loading empty error and stale states", async ({ page }) => {
  await mockAuth(page, { admin: false, usage: true })
  let releaseOverview!: () => void
  const overviewBlocked = new Promise<void>((resolve) => {
    releaseOverview = resolve
  })
  await page.route("**/api/usage/overview**", async (r) => {
    await overviewBlocked
    await r.fulfill({
      json: { data_as_of: "2026-08-10T00:00:00Z", partial: false, pools: [] },
    })
  })
  await page.goto("/usage")
  await expect(page.getByText("Loading usage data")).toBeVisible()
  releaseOverview()
  await expect(
    page.getByText("No usage data is available for this timeframe."),
  ).toBeVisible()
  await expect(page.getByText("Usage data may be stale")).toBeVisible()
})
