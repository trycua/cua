import { resolve } from "node:path"
import { chromium } from "@playwright/test"

const root = resolve(process.cwd(), "..")
const pageUrl = `file://${resolve(
  root,
  "docs/superpowers/evidence/2026-08-16-settings-checkpoint/settings-composition.html",
)}`
const output = resolve(
  root,
  "docs/superpowers/evidence/2026-08-16-settings-checkpoint",
)
const browser = await chromium.launch({
  headless: true,
  executablePath:
    process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ??
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
})

for (const [name, width, height] of [
  ["settings-desktop-1440", 1440, 1200],
  ["settings-phone-375", 375, 1000],
]) {
  const page = await browser.newPage({ viewport: { width, height } })
  await page.goto(pageUrl)
  await page.evaluate(() => document.fonts.ready)
  await page.screenshot({
    path: resolve(output, `${name}.png`),
    fullPage: true,
  })
  await page.close()
}

await browser.close()
console.log(output)
