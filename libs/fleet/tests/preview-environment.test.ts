import assert from "node:assert/strict"
import test from "node:test"
import { isReviewPreviewHostname } from "../src/preview-environment"

test("recognizes Cyclops CS pull request preview hostnames", () => {
  assert.equal(
    isReviewPreviewHostname("cyclops-cs-pr-123.tail204509.ts.net"),
    true,
  )
})

test("rejects production and lookalike hostnames", () => {
  assert.equal(isReviewPreviewHostname("cyclops.cua.ai"), false)
  assert.equal(
    isReviewPreviewHostname("cyclops-cs-pr-123.example.com"),
    false,
  )
  assert.equal(
    isReviewPreviewHostname("evil-cyclops-cs-pr-123.tail204509.ts.net"),
    false,
  )
})
