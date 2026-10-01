// The New Space plan to SDK call mapping (hermetic: no SDK, no network).
import assert from "node:assert/strict"
import { readFileSync } from "node:fs"
import { test } from "node:test"
import { IMAGE_LIST_PATH, planCall, publishedImages } from "../dist/core/plan.js"

const plan = (image, target, extra = {}) => ({ image, target, name: "koala-desk", cpus: 4, memory_mb: 8192, ...extra })

test("reads the published entries of the shared list, in order", () => {
  const file = JSON.parse(readFileSync(IMAGE_LIST_PATH, "utf8"))
  assert.deepEqual(publishedImages().map((i) => i.ref), file.images.filter((i) => i.published).map((i) => i.ref))
})

test("local plans name the location, kind, engine and resources", () => {
  assert.deepEqual(planCall(plan("ghcr.io/trycua/linux:24.04", "local")), {
    on: "local", image: "ghcr.io/trycua/linux:24.04", kind: "container", runtime: "auto", name: "koala-desk", cpus: 4, memoryMb: 8192, wait: true, spacesd: true,
  })
  const vm = planCall(plan("ghcr.io/trycua/linux:24.04-disk", "local"))
  assert.deepEqual([vm.image, vm.kind, vm.runtime], ["ghcr.io/trycua/linux:24.04-disk", "vm", "qemu"])
  const mac = planCall(plan("ghcr.io/trycua/macos:26", "local"))
  assert.deepEqual([mac.kind, mac.runtime], ["vm", "lume"])
})

test("spacesd comes from the catalog entry, never the OS", () => {
  const file = JSON.parse(readFileSync(IMAGE_LIST_PATH, "utf8"))
  for (const entry of file.images.filter((i) => i.published)) {
    const target = entry.local ? "local" : "cloud"
    assert.equal(planCall(plan(entry.ref, target)).spacesd, entry.spacesd, entry.ref)
  }
  // macos:26 ships cua-spacesd (catalog `spacesd: true`).
  assert.equal(planCall(plan("ghcr.io/trycua/macos:26", "local")).spacesd, true)
  // A list that says otherwise wins: the plan reads the entry.
  const images = publishedImages().map((i) => (i.ref === "ghcr.io/trycua/macos:26" ? { ...i, spacesd: false } : i))
  assert.equal(planCall(plan("ghcr.io/trycua/macos:26", "local"), images).spacesd, false)
})

test("cloud plans create in Cua Cloud with the entry's engine", () => {
  assert.deepEqual(planCall(plan("ghcr.io/trycua/linux:24.04", "cloud")), {
    on: "cloud", image: "ghcr.io/trycua/linux:24.04", kind: "container", runtime: "gvisor", name: "koala-desk", wait: true, spacesd: true,
  })
  const w = planCall(plan("ghcr.io/trycua/windows:2022", "cloud"))
  assert.deepEqual([w.kind, w.runtime], ["vm", "kubevirt"])
})

test("refuses unsupported targets, unpublished images and bad names", () => {
  assert.throws(() => planCall(plan("ghcr.io/trycua/macos:26", "cloud")), /does not run in Cua Cloud/)
  assert.throws(() => planCall(plan("ghcr.io/trycua/bench-web:1.0", "local")), /not a published image/)
  assert.throws(() => planCall(plan("ghcr.io/trycua/linux:24.04", "local", { name: "Koala Desk" })), /DNS label/)
})

test("resources default and clamp", () => {
  const c = planCall(plan("ghcr.io/trycua/linux:24.04", "local", { cpus: null, memory_mb: 1 }))
  assert.equal(c.cpus, 2)
  assert.equal(c.memoryMb, 1024)
})
