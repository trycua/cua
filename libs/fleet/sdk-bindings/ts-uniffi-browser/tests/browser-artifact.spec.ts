import { expect, test } from "@playwright/test"
import { access } from "node:fs/promises"
import { fileURLToPath } from "node:url"

const bindingRoot = fileURLToPath(new URL("..", import.meta.url))
const requiredArtifacts = [
  new URL("../ts/wasm-bindgen/index.js", import.meta.url),
  new URL("../ts/wasm-bindgen/index_bg.wasm", import.meta.url),
]

test("loads the Vite bundle backed by the generated WASM SDK artifact", async ({
  page,
}) => {
  for (const artifact of requiredArtifacts) {
    await expect(access(fileURLToPath(artifact))).resolves.toBeUndefined()
  }

  await page.goto("/")
  await expect(page.getByTestId("sdk-ready")).toHaveText("ready")
})
