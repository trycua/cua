import assert from "node:assert/strict"
import test from "node:test"
import { readFile } from "node:fs/promises"

test("claim detail manages signed service URLs through the shared SDK", async () => {
  const source = [
    await readFile(new URL("../src/pages/ClaimDetail.tsx", import.meta.url), "utf8"),
    await readFile(new URL("../src/components/SignedServiceUrls.tsx", import.meta.url), "utf8"),
  ].join("\n")

  assert.match(source, /CreateSignedServiceUrlRequestBuilder/)
  assert.match(source, /client\.createSignedServiceUrl\(/)
  assert.match(source, /client\.listSignedServiceUrls\(/)
  assert.match(source, /client\.revokeSignedServiceUrl\(/)
  assert.doesNotMatch(source, /createPublicUrl|listPublicUrls|deletePublicUrl|Public URL/)
})

test("services retain authenticated proxy links", async () => {
  const source = await readFile(new URL("../src/pages/ClaimDetail.tsx", import.meta.url), "utf8")

  assert.match(source, /\/api\/svc\/\$\{namespace\}\/\$\{svcK8sName\}\//)
})

test("signed URL collection guards stale responses and disables on typed unavailability", async () => {
  const source = await readFile(new URL("../src/components/SignedServiceUrls.tsx", import.meta.url), "utf8")

  assert.match(source, /useRef/)
  assert.match(source, /requestGeneration/)
  assert.match(source, /SdkError\.SignedServiceUrlsUnavailable\.instanceOf\(error\)/)
  assert.match(source, /client\.createSignedServiceUrl\(requestBuilder\.build\(\)\)/)
  assert.match(source, /client\.revokeSignedServiceUrl\(item\)/)
  assert.match(source, /copyButtonAriaLabel/)
})
