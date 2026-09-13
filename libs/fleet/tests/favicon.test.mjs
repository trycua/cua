import test from "node:test"
import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { fileURLToPath } from "node:url"
import path from "node:path"

const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)
const cyclopsRoot = path.resolve(__dirname, "..")
const websitePublic = path.resolve(cyclopsRoot, "../src/website/public")

const faviconLinks = [
  '<link rel="icon" href="/favicon.svg" type="image/svg+xml" sizes="any" />',
  '<link rel="icon" href="/favicon.ico" sizes="32x32" />',
  '<link rel="icon" href="/favicon.png" type="image/png" />',
  '<link rel="apple-touch-icon" href="/apple-touch-icon.png" />',
]

const faviconAssets = [
  "favicon.svg",
  "favicon.ico",
  "favicon.png",
  "apple-touch-icon.png",
]

test("frontend declares the shared Cua favicon links", async () => {
  const indexHtml = await readFile(path.join(cyclopsRoot, "index.html"), "utf8")

  for (const link of faviconLinks) {
    assert.ok(indexHtml.includes(link), `expected index.html to include ${link}`)
  }
})

test("frontend favicon assets match cua.ai byte-for-byte", async () => {
  for (const asset of faviconAssets) {
    const [source, destination] = await Promise.all([
      readFile(path.join(websitePublic, asset)),
      readFile(path.join(cyclopsRoot, "public", asset)),
    ])

    assert.deepEqual(destination, source, `${asset} differs from cua.ai`)
  }
})
