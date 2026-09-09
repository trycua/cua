import test from "node:test"
import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { fileURLToPath } from "node:url"
import path from "node:path"

const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)
const nginxConf = path.resolve(__dirname, "../nginx.conf")
const backendDeployment = path.resolve(
  __dirname,
  "../../clusters/kopf-k3s/cyclops-cs/backend-deployment.yaml",
)

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
  assert.match(
    backendRoute[1],
    /\bstate\b/,
    "expected /api/state/query to proxy to cyclops-cs-backend",
  )
  assert.match(
    backendRoute[1],
    /\busage\b/,
    "expected /api/usage to proxy to cyclops-cs-backend",
  )
  assert.match(
    backendRoute[1],
    /\badmin\b/,
    "expected /api/admin/feature-flags to proxy to cyclops-cs-backend",
  )
  assert.match(
    backendRoute[1],
    /\bsigned-service-urls\b/,
    "expected /api/signed-service-urls to proxy to cyclops-cs-backend",
  )
})

test("oauth2-proxy accepts Fleets GitHub WIF bearer tokens", async () => {
  const deployment = await readFile(backendDeployment, "utf8")

  assert.match(
    deployment,
    /--extra-jwt-issuers=https:\/\/token\.actions\.githubusercontent\.com=fleets/,
    "expected oauth2-proxy to verify GitHub OIDC tokens with the Fleets audience",
  )
})

test("sandbox service proxy streams bodies without a size cap", async () => {
  const conf = await readFile(nginxConf, "utf8")
  const route = conf.match(/location ~ \^\/api\/svc\(\/\|\$\) \{([\s\S]*?)\n    \}/)
  assert.ok(route)
  assert.match(route[1], /client_max_body_size\s+0\s*;/)
  assert.match(route[1], /proxy_request_buffering\s+off\s*;/)
  assert.match(route[1], /proxy_buffering\s+off\s*;/)
  assert.match(route[1], /proxy_read_timeout\s+1h\s*;/)
  assert.match(route[1], /proxy_send_timeout\s+1h\s*;/)
})

test("signed sandbox service proxy bypasses OAuth and SPA fallback", async () => {
  const conf = await readFile(nginxConf, "utf8")
  const route = conf.match(/location \^~ \/api\/signed-svc\/ \{([\s\S]*?)\n    \}/)

  assert.ok(route, "expected /api/signed-svc/ proxy route in nginx.conf")
  assert.match(route[1], /access_log\s+off\s*;/)
  assert.match(route[1], /error_log\s+\/dev\/null\s+crit\s*;/)
  assert.match(route[1], /proxy_pass\s+\$\{CYCLOPS_CS_BACKEND\}\s*;/)
  assert.doesNotMatch(route[1], /auth_request|oauth2|\$request_uri/)
  assert.match(route[1], /client_max_body_size\s+0\s*;/)
  assert.match(route[1], /proxy_request_buffering\s+off\s*;/)
  assert.match(route[1], /proxy_buffering\s+off\s*;/)
})
