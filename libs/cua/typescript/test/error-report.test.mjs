import assert from "node:assert/strict"
import { test } from "node:test"

import * as cua from "../dist/index.js"

test("an error has a message, a code and a hint", () => {
  const e = new cua.CuaError.NotFound("sandbox nope")
  assert.match(e.message, /sandbox nope/)
  assert.equal(e.code, "not_found")
  assert.match(e.hint, /cua sb ls/)
  assert.equal(e.report.message, "sandbox nope")
  // Node prints the message, not just the class name.
  assert.match(String(e.stack), /sandbox nope/)
})

test("a taken name is its own error", () => {
  const e = new cua.CuaError.AlreadyExists("a sandbox named 'x' already exists")
  assert.equal(e.code, "already_exists")
  assert.match(e.hint, /cua sb rm x/)
})

test("layers are cut from the message", () => {
  const e = new cua.CuaError.Runtime(
    "[local] container engine: pull linux: Docker responded with status code 404: pull access denied for linux, repository does not exist",
  )
  assert.equal(e.code, "image_not_found")
  assert.equal(e.report.message, 'no image named "linux"')
})

test("sandboxes have delete as well as delete_", () => {
  assert.equal(typeof cua.Sandboxes.prototype.delete, "function")
  assert.equal(typeof cua.Sandbox.prototype.delete, "function")
})

test("output is off until asked for", () => {
  assert.equal(typeof cua.enableOutput, "function")
  cua.enableOutput()
  cua.disableOutput()
})

test("create takes an onProgress callback", async () => {
  const seen = []
  const fake = {
    createWithProgress: async (_o, listener) => {
      listener.onProgress({ phase: "preparing" })
      return "sandbox"
    },
  }
  const r = await cua.Sandboxes.prototype.create.call(fake, {}, { onProgress: (p) => seen.push(p.phase) })
  assert.equal(r, "sandbox")
  assert.deepEqual(seen, ["preparing"])
})
