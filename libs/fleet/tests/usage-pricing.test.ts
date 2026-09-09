import assert from "node:assert/strict"
import test from "node:test"
import {
  DEFAULT_USAGE_PRICING,
  reservedResourceCostUSD,
} from "../src/usagePricing.ts"

test("reservation pricing defaults to the current billable rates", () => {
  assert.equal(DEFAULT_USAGE_PRICING.vcpuHourUSD, 0.044625)
  assert.equal(DEFAULT_USAGE_PRICING.memoryGiBHourUSD, 0.0223125)
})

test("reservation cost uses runtime pricing", () => {
  assert.equal(
    reservedResourceCostUSD(28, 28, { vcpuHourUSD: 0.1, memoryGiBHourUSD: 0.2 }),
    8.4,
  )
})
