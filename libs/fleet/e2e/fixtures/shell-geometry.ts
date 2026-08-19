import { expect, type Page } from "@playwright/test"

export async function expectSharedPageShell(page: Page): Promise<void> {
  const header = page.locator(".cua-pagehead")
  const body = page.locator(".cua-pagebody")
  await expect(header).toBeVisible()
  await expect(body).toBeVisible()

  const [headerBox, bodyBox] = await Promise.all([
    header.boundingBox(),
    body.boundingBox(),
  ])
  expect(headerBox).not.toBeNull()
  expect(bodyBox).not.toBeNull()
  expect(Math.abs(headerBox!.x - bodyBox!.x)).toBeLessThanOrEqual(1)
  expect(
    Math.abs(
      headerBox!.x + headerBox!.width - (bodyBox!.x + bodyBox!.width),
    ),
  ).toBeLessThanOrEqual(1)
  expect(
    await page.locator("main").evaluate(
      element => element.scrollWidth - element.clientWidth,
    ),
  ).toBeLessThanOrEqual(1)
}
