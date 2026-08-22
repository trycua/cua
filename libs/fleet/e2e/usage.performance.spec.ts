import { expect, test, type Page, type Route } from "@playwright/test"
import { mockAuth } from "./fixtures/mock-api"

const overview = {
  data_as_of: "2026-08-22T04:00:00Z",
  partial: false,
  pools: [
    {
      id: "ns-a:pool-a",
      name: "Pool A",
      cpu: { consumed: 12, provisioned: 24 },
      memory: { consumed: 48, provisioned: 96 },
      cost_usd: 2.75,
    },
  ],
}

async function delayedJSON(route: Route, body: unknown) {
  await new Promise((resolve) => setTimeout(resolve, 200))
  await route.fulfill({ json: body })
}

test("measures Usage dashboard ready time", async ({ page }) => {
  const visualPreview = process.env.CYCLOPS_BROWSER_VISUAL_PREVIEW === "true"
  if (!visualPreview) await mockAuth(page, { admin: false, billing: false })
  let usageRequests = 0
  let browserTimings: Record<string, number> | null = null
  await page.route("**/api/usage/overview**", async (route) => {
    usageRequests++
    await delayedJSON(route, overview)
  })
  await page.route("**/api/usage/browser-timings**", async (route) => {
    browserTimings = route.request().postDataJSON() as Record<string, number>
    await route.fulfill({ status: 204 })
  })

  const started = performance.now()
  await page.goto(visualPreview ? "/usage?cua-visual-preview" : "/usage")
  await expect(
    page.getByRole("heading", { name: "Cost by pool" }),
  ).toBeVisible()
  const readyMS = performance.now() - started
  await expect.poll(() => browserTimings).not.toBeNull()

  console.log(`METRIC browser_ready_ms=${readyMS.toFixed(3)}`)
  console.log(`METRIC browser_usage_requests=${usageRequests}`)
  expect(usageRequests).toBe(1)
  expect(browserTimings?.initial_load_ms).toBeGreaterThanOrEqual(200)
  expect(browserTimings?.dashboard_ready_ms).toBeGreaterThanOrEqual(200)
})
