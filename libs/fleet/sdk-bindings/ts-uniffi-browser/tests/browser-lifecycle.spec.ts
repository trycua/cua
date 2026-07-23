import { expect, test } from "@playwright/test"

test("uses runner-injected access tokens without browser credential fields", async ({
  page,
}) => {
  await page.addInitScript(() => {
    window.__CYCLOPS_BROWSER_CONFIG__ = {
      accessToken: "browser-token",
      baseUrl: "https://run.example",
      namespace: "browser-sdk-test",
      image: "registry.example/desktop:latest",
      imagePullSecret: "ecr-credentials",
    }
  })

  await page.goto("/")

  await expect(page.getByTestId("sdk-ready")).toHaveText("ready")
  await expect(page.getByRole("button", { name: "Run Lifecycle" })).toBeVisible()
  await expect(page.locator("input[name=clientSecret]")).toHaveCount(0)
  await expect(page.locator("input[name=clientId]")).toHaveCount(0)
  await expect(page.locator("input[name=tokenUrl]")).toHaveCount(0)
})
