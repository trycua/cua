import assert from "node:assert/strict"
import { readFile } from "node:fs/promises"
import { join } from "node:path"
import test from "node:test"

import { CuaDriverClient, normalizeToolResult } from "../dist/index.js"

const fixtures = join(import.meta.dirname, "..", "..", "..", "contract", "fixtures")
const fixture = async name => JSON.parse(await readFile(join(fixtures, name), "utf8")).result

class FakeTransport {
  calls = []
  constructor(response) {
    this.response = response
  }
  async request(method, params) {
    this.calls.push([method, params])
    return this.response
  }
  async close() {}
}

test("generated session method converts camelCase to wire names", async () => {
  const transport = new FakeTransport(await fixture("session-success.json"))
  const client = new CuaDriverClient(transport)
  const result = await client.startSession({ session: "demo", captureScope: "auto" })
  assert.equal(result.structured.session, "demo")
  assert.deepEqual(transport.calls, [
    [
      "tools/call",
      {
        name: "start_session",
        arguments: { session: "demo", capture_scope: "auto" },
      },
    ],
  ])
})

test("normalizes images and structured refusals", async () => {
  const image = normalizeToolResult(await fixture("image-result.json"))
  assert.equal(image.text, "captured")
  assert.equal(image.images[0].mimeType, "image/png")
  const refused = normalizeToolResult(await fixture("tool-refusal.json"))
  assert.equal(refused.isError, true)
  assert.equal(refused.errorCode, "foreground_required")
})
