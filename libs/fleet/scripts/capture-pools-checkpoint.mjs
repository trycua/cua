import { mkdir } from "node:fs/promises"
import { resolve } from "node:path"
import { chromium } from "@playwright/test"

const output = resolve(
  process.cwd(),
  "../docs/superpowers/evidence/2026-08-16-pools-checkpoint-b",
)
await mkdir(output, { recursive: true })

const browser = await chromium.launch({
  headless: true,
  executablePath:
    process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ??
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
})

async function open(width, height = 900, path = "/pools?cua-visual-preview") {
  const page = await browser.newPage({ viewport: { width, height } })
  await page.goto(`http://127.0.0.1:5180${path}`)
  await page.locator("body#cua-dashboard-root").waitFor()
  return page
}

async function capture(name, page) {
  await page.waitForTimeout(250)
  await page.screenshot({ path: resolve(output, `${name}.png`) })
  await page.close()
}

for (const [name, width] of [
  ["desktop-1440", 1440],
  ["tablet-768", 768],
  ["phone-375", 375],
  ["compact-320", 320],
]) {
  const page = await open(width)
  await page.getByRole("heading", { name: "Pools" }).waitFor()
  await capture(name, page)
}

{
  const page = await open(1440, 900, "/pools?cua-visual-preview&cua-preview-state=loading")
  await page.getByText("Loading pools").waitFor()
  await capture("state-loading", page)
}

{
  const page = await open(1440, 900, "/pools?cua-visual-preview&cua-preview-state=empty")
  await page.getByRole("heading", { name: "No pools" }).waitFor()
  await capture("state-empty", page)
}

{
  const page = await open(1440)
  const filter = page.getByPlaceholder("Filter pools by property")
  await filter.fill("pool-that-does-not-exist")
  await filter.press("Enter")
  await page.getByRole("heading", { name: "No matches" }).waitFor()
  await capture("state-no-matches", page)
}

{
  const page = await open(1440)
  const row = page.getByRole("row").filter({ hasText: "prod-web-fleet" })
  await row.getByRole("checkbox").check()
  await page.getByText("1 pool selected").waitFor()
  await page.screenshot({ path: resolve(output, "state-selected.png") })
  await page.getByRole("button", { name: "Delete" }).click()
  await page.getByRole("dialog", { name: "Delete 1 pool?" }).waitFor()
  await capture("state-delete-confirmation", page)
}

{
  const page = await open(375)
  await page.getByRole("button", { name: "Open navigation" }).click()
  await page.getByRole("navigation", { name: "Main navigation" }).waitFor()
  await capture("state-mobile-navigation", page)
}

{
  const page = await open(1440)
  await page.getByRole("button", { name: "Preview" }).click()
  await page.getByRole("menuitem", { name: "Sign out" }).waitFor()
  await capture("state-account-menu", page)
}

await browser.close()
console.log(output)
