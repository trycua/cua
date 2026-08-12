import { expect, test } from "@playwright/test"

test.describe("CLI device authorization", () => {
  test("continues sign-in and signup from a normalized user code", async ({ page }) => {
    await page.goto("/device?user_code=abcd-efgh")

    await expect(
      page.getByRole("heading", { name: "Connect your Cua CLI" }),
    ).toBeVisible()
    await expect(page.getByText("ABCD-EFGH", { exact: true })).toBeVisible()

    const signIn = page.getByRole("link", { name: "Sign in and continue" })
    await expect(signIn).toHaveAttribute(
      "href",
      "https://auth.cua.ai/realms/cyclops-cs/device?user_code=ABCD-EFGH",
    )

    const signUp = page.getByRole("link", { name: "Create an account" })
    const signUpHref = await signUp.getAttribute("href")
    expect(signUpHref).toBe(
      "https://cua.ai/signup?redirect_url=%2Fdevice%3Fuser_code%3DABCD-EFGH",
    )
  })

  test("rejects missing or malformed user codes", async ({ page }) => {
    await page.goto("/device?user_code=not-a-code")

    await expect(page.getByRole("alert")).toContainText(
      "Enter the code shown by the CLI",
    )
    await expect(
      page.getByRole("link", { name: "Sign in and continue" }),
    ).toHaveCount(0)
    await expect(
      page.getByRole("link", { name: "Create an account" }),
    ).toHaveCount(0)
  })
})
