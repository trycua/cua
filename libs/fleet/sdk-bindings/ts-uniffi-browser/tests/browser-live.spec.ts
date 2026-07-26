import { expect, test, type Page, type Route } from "@playwright/test"
import { readdir } from "node:fs/promises"
import { join } from "node:path"
import { fileURLToPath } from "node:url"

const bindingRoot = fileURLToPath(new URL("..", import.meta.url))
const distRoot = join(bindingRoot, "examples", "dist")

test.describe.configure({ mode: "serial" })
test.use({ trace: "off" })

test("runs the browser SDK lifecycle with a runner-injected access token", async ({
  page,
}) => {
  test.setTimeout(15 * 60_000)
  const config = requiredRuntimeConfig()

  const networkEvents: string[] = []
  const browserDiagnostics: string[] = []
  page.on("console", (message) => {
    if (message.type() === "error") {
      browserDiagnostics.push(`console error: ${message.text()}`)
    }
  })
  page.on("pageerror", (error) => {
    browserDiagnostics.push(`page error: ${error.stack ?? error.message}`)
  })
  page.on("request", (request) => {
    if (request.url().startsWith(`${config.baseUrl}/api/`)) {
      networkEvents.push(`request ${request.method()} ${new URL(request.url()).pathname}`)
    }
  })
  page.on("response", (response) => {
    if (response.url().startsWith(`${config.baseUrl}/api/`)) {
      networkEvents.push(`response ${response.status()} ${new URL(response.url()).pathname}`)
    }
  })
  page.on("requestfailed", (request) => {
    if (request.url().startsWith(`${config.baseUrl}/api/`)) {
      networkEvents.push(
        `request failed ${new URL(request.url()).pathname}: ${request.failure()?.errorText ?? "unknown error"}`,
      )
    }
  })

  await serveBundleFromControlPlaneOrigin(page, config.baseUrl)
  await page.addInitScript((runtimeConfig) => {
    window.__CYCLOPS_BROWSER_CONFIG__ = runtimeConfig
  }, config)
  await page.goto(new URL("/__cyclops-browser-sdk-live-test/", config.baseUrl).toString())

  await expect(page.getByTestId("sdk-ready")).toHaveText("ready")
  await page.getByRole("button", { name: "Run Lifecycle" }).click()

  const lifecycleStatus = page.getByTestId("lifecycle-status")
  await expect(lifecycleStatus).toHaveText(/^(completed|failed)$/, {
    timeout: 12 * 60_000,
  })
  if ((await lifecycleStatus.textContent()) !== "completed") {
    const output = await page.locator("#output").textContent()
    throw new Error(
      `browser lifecycle failed:\n${output ?? "no output"}\nnetwork:\n${networkEvents.join("\n") || "no control-plane requests"}\nbrowser diagnostics:\n${browserDiagnostics.join("\n") || "none"}`,
    )
  }
})

async function serveBundleFromControlPlaneOrigin(
  page: Page,
  baseUrl: string,
): Promise<void> {
  const assets = await readdir(join(distRoot, "assets"))
  const wasmFile = assets.find((file) => file.endsWith(".wasm"))
  if (!wasmFile) {
    throw new Error("generated browser bundle does not contain a WASM asset")
  }

  await page.route(new URL("/__cyclops-browser-sdk-live-test/", baseUrl).toString(), (route: Route) =>
    route.fulfill({ path: join(distRoot, "index.html") }),
  )
  await page.route(new URL("/browser.js", baseUrl).toString(), (route: Route) =>
    route.fulfill({ path: join(distRoot, "browser.js") }),
  )
  await page.route(new URL(`/assets/${wasmFile}`, baseUrl).toString(), (route: Route) =>
    route.fulfill({ path: join(distRoot, "assets", wasmFile) }),
  )
}

function requiredRuntimeConfig(): {
  accessToken: string
  baseUrl: string
  namespace: string
  image: string
  imagePullSecret: string
} {
  const values = {
    accessToken: process.env.CYCLOPS_ACCESS_TOKEN,
    baseUrl: process.env.CUA_BASE_URL,
    namespace: process.env.CYCLOPS_NAMESPACE,
    image: process.env.CUA_IMAGE,
    imagePullSecret: process.env.CUA_IMAGE_PULL_SECRET,
  }

  for (const [name, value] of Object.entries(values)) {
    if (!value) {
      throw new Error(`missing required environment variable: ${name}`)
    }
  }
  return values as {
    accessToken: string
    baseUrl: string
    namespace: string
    image: string
    imagePullSecret: string
  }
}
