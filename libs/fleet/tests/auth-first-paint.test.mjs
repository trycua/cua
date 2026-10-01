import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import test from "node:test"
import path from "node:path"
import { fileURLToPath } from "node:url"

const testDirectory = path.dirname(fileURLToPath(import.meta.url))
const appRoot = path.resolve(testDirectory, "..")

test("the auth handoff is present before React without a black or monospace frame", async () => {
  const [index, provider] = await Promise.all([
    readFile(path.join(appRoot, "index.html"), "utf8"),
    readFile(path.join(appRoot, "src/auth/AuthProvider.tsx"), "utf8"),
  ])

  assert.match(index, /<meta name="theme-color" content="#07080a"/)
  assert.match(index, /id="cua-boot"/)
  assert.match(index, /Signing you in/)
  assert.match(index, /Continuing to auth\.cua\.ai/)
  assert.match(index, /350ms/)
  assert.match(index, /radial-gradient/)
  assert.doesNotMatch(index, /Pools · Cua/)

  assert.match(provider, /if \(!ready\) return null/)
  assert.match(provider, /document\.getElementById\("cua-boot"\)\?\.remove\(\)/)
  assert.match(provider, /We couldn&apos;t sign you in/)
  assert.doesNotMatch(provider, /fontFamily: "monospace"/)
  assert.doesNotMatch(provider, /Signing in&hellip;/)
})
