import assert from "node:assert/strict"
import test from "node:test"
import { SDK, UnknownServiceError } from "../dist/index.js"

test("serviceFetch rejects unknown services with sorted available names", () => {
  const sdk = Object.create(SDK.prototype)
  assert.throws(
    () => sdk.serviceFetch({ namespace: "pool-one", claim: "claim-one", sandbox: "sandbox-one", services: ["novnc", "mcp", "mcp"] }, "metrics"),
    error => {
      assert.ok(error instanceof UnknownServiceError)
      assert.equal(error.requested, "metrics")
      assert.deepEqual(error.available, ["mcp", "novnc"])
      return true
    },
  )
})
