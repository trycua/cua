import assert from "node:assert/strict"
import test from "node:test"
import { errorMessage } from "../src/error-message.ts"

// Structural stand-ins for uniffi enum errors (e.g. the fleet SDK's
// SdkError): a real one is an Error whose message is just the variant name
// ("SdkError.Status") with the details frozen on `inner`.
function uniffiError(
  variant: string,
  inner: Record<string, unknown>,
): Error & { tag: string; inner: Record<string, unknown> } {
  return Object.assign(new Error(`SdkError.${variant}`), {
    tag: variant,
    inner,
  })
}

test("pipes the auth middleware's {error} JSON body through Status errors", () => {
  const denial =
    "A payment method is required to create this resource. Add one in Billing and try again."
  const error = uniffiError("Status", {
    operation: "createPool",
    status: 403,
    body: JSON.stringify({ error: denial }),
  })
  assert.equal(errorMessage(error), denial)
})

test("pipes the K8s API's {message} JSON body", () => {
  const error = uniffiError("Status", {
    operation: "createPool",
    status: 409,
    body: JSON.stringify({
      kind: "Status",
      status: "Failure",
      message: 'warmpools.osgym.cua.ai "demo" already exists',
      reason: "AlreadyExists",
    }),
  })
  assert.equal(errorMessage(error), 'warmpools.osgym.cua.ai "demo" already exists')
})

test("passes a non-JSON body through as-is", () => {
  const error = uniffiError("Status", {
    operation: "createPool",
    status: 502,
    body: "upstream connect error",
  })
  assert.equal(errorMessage(error), "upstream connect error")
})

test("falls back to the HTTP status when the body is empty", () => {
  const error = uniffiError("Status", {
    operation: "createPool",
    status: 403,
    body: "   ",
  })
  assert.equal(errorMessage(error), "Request failed with HTTP status 403.")
})

test("handles PoolAccessDenied's {status, body} payload like Status", () => {
  const error = uniffiError("PoolAccessDenied", {
    operation: "createPool",
    namespace: "demo",
    status: 403,
    body: JSON.stringify({ error: "namespace demo is not yours" }),
  })
  assert.equal(errorMessage(error), "namespace demo is not yours")
})

test("surfaces reason-only variants like Transport", () => {
  const error = uniffiError("Transport", { reason: "dns lookup failed" })
  assert.equal(errorMessage(error), "dns lookup failed")
})

test("keeps a variant with neither status nor reason at its Error message", () => {
  const error = uniffiError("ClaimTimeout", {})
  assert.equal(errorMessage(error), "SdkError.ClaimTimeout")
})

test("leaves plain errors, strings, and nullish values unchanged", () => {
  assert.equal(errorMessage(new Error("boom")), "boom")
  assert.equal(errorMessage("plain failure"), "plain failure")
  assert.equal(errorMessage(null), "Unknown error")
  assert.equal(errorMessage(undefined), "Unknown error")
})
