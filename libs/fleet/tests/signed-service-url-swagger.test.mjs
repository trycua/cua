import assert from "node:assert/strict"
import test from "node:test"
import { readFile } from "node:fs/promises"

function definition(source, name) {
  const start = source.indexOf(`  ${name}:`)
  assert.notEqual(start, -1, `${name} definition is missing`)
  const next = source.indexOf("\n  handlers.", start + name.length + 4)
  return source.slice(start, next === -1 ? undefined : next)
}

test("signed service URL Swagger documents request bounds and server failures", async () => {
  const source = await readFile(new URL("../backend/docs/swagger.yaml", import.meta.url), "utf8")
  const request = definition(source, "handlers.CreateSignedServiceURLRequest")

  assert.match(request, /expiresInSeconds:[\s\S]*maximum: 86400[\s\S]*minimum: 60/)
  assert.match(request, /label:[\s\S]*maxLength: 120/)
  assert.match(request, /120 UTF-8 bytes[\s\S]*backend byte validation remains authoritative/)
  for (const field of ["claim", "sandbox", "service", "logicalService", "expiresInSeconds"]) {
    assert.ok(request.split("\n").includes(`    - ${field}`), `${field} is required`)
  }

  const collection = source.slice(source.indexOf("  /api/signed-service-urls/{namespace}:"), source.indexOf("  /api/signed-service-urls/{namespace}/{id}:"))
  assert.match(collection, /get:[\s\S]*"500":/)
  assert.match(collection, /post:[\s\S]*"500":/)
  assert.match(source.slice(source.indexOf("  /api/signed-service-urls/{namespace}/{id}:")), /delete:[\s\S]*"500":/)
})
