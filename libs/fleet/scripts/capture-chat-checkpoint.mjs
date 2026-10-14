import { mkdir } from "node:fs/promises"
import { resolve } from "node:path"
import { chromium } from "@playwright/test"

const output = resolve(
  process.cwd(),
  "../docs/superpowers/evidence/2026-08-16-chat-checkpoint",
)
await mkdir(output, { recursive: true })

const browser = await chromium.launch({
  headless: true,
  executablePath:
    process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH ??
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
})

async function open(width, height, suffix = "") {
  const page = await browser.newPage({ viewport: { width, height } })
  await page.goto(
    `http://127.0.0.1:5180/agent?cua-visual-preview${suffix}`,
  )
  await page.getByRole("heading", { name: "Chat" }).waitFor()
  return page
}

async function capture(name, page) {
  await page.screenshot({ path: resolve(output, `${name}.png`) })
  await page.close()
}

{
  const page = await open(1440, 900)
  await capture("desktop-empty-1440", page)
}

{
  const page = await open(1440, 900)
  await page
    .getByRole("button", { name: "Inspect the browser fleet" })
    .click()
  await page.getByText("Command completed").waitFor()
  await capture("desktop-active-1440", page)
}

{
  const page = await open(1440, 900, "&cua-preview-state=loading")
  await page.getByTestId("conversation-skeleton").first().waitFor()
  await capture("desktop-loading-1440", page)
}

{
  const page = await open(390, 844)
  await page.getByRole("button", { name: "View conversations" }).click()
  await page.getByRole("dialog", { name: "Conversations" }).waitFor()
  await capture("mobile-drawer-390", page)
}

{
  const page = await open(390, 844)
  await page
    .getByPlaceholder("Ask a question")
    .fill("Check the browser fleet\nand compare availability\nwith the staging pool\nbefore proposing next steps.")
  await capture("mobile-long-composer-390", page)
}

await browser.close()
console.log(output)
