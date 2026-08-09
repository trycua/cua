import test from "node:test"
import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { fileURLToPath } from "node:url"
import path from "node:path"

const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)
const nginxConf = path.resolve(__dirname, "../nginx.conf")

test("backend API nginx route includes GitHub trust policy endpoints", async () => {
  const conf = await readFile(nginxConf, "utf8")
  const backendRoute = conf.match(
    /location ~ \^\/api\/\(([^)]+)\)\(\/\|\$\) \{/,
  )

  assert.ok(backendRoute, "expected backend API proxy route in nginx.conf")
  assert.match(
    backendRoute[1],
    /\bgithub-trust-policies\b/,
    "expected /api/github-trust-policies to proxy to cyclops-cs-backend",
  )
})
