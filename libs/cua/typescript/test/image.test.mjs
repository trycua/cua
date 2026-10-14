// Canonical images and normalisation through the native resolver (no
// registry read: only the pure helpers run here).
import assert from "node:assert/strict"
import { test } from "node:test"

import * as cua from "../dist/index.js"

test("Image aliases map to the canonical ghcr.io/trycua refs", () => {
  const saved = process.env.CUA_IMAGE_LINUX
  delete process.env.CUA_IMAGE_LINUX
  try {
    assert.equal(cua.Image.linux(), "ghcr.io/trycua/linux:24.04")
    assert.equal(cua.Image.windows(), "ghcr.io/trycua/windows:2022")
    assert.equal(cua.Image.macos(), "ghcr.io/trycua/macos:26")
    assert.equal(cua.Image.macos("sequoia"), "ghcr.io/trycua/macos:15")
    // fromRegistry is literal; aliases are CLI words only.
    assert.equal(cua.Image.fromRegistry("ubuntu:24.04"), "ubuntu:24.04")
    assert.equal(cua.imageAlias("ubuntu:24.04"), undefined)
    assert.equal(cua.imageAlias("macos:tahoe"), "ghcr.io/trycua/macos:26")
    assert.equal(cua.Image.fromRegistry("python:3.12-slim"), "python:3.12-slim")
    assert.equal(cua.normalizeImage("python:3.12-slim"), "docker.io/library/python:3.12-slim")
    assert.throws(() => cua.resolveImage("x", "kvm", undefined))
  } finally {
    if (saved !== undefined) process.env.CUA_IMAGE_LINUX = saved
  }
})

// Tiers and Omarchy: unpublished catalog entries raise ImageNotPublished.
import { readFileSync } from "node:fs"
const catalog = JSON.parse(
  readFileSync(new URL("../../../images/sandbox-images.json", import.meta.url), "utf8"),
)
const published = (ref) => catalog.images.find((i) => i.ref === ref)?.published ?? true
const expectRef = (fn, ref) => {
  if (published(ref)) {
    assert.equal(fn(), ref)
  } else {
    assert.throws(fn, (e) => {
      assert.equal(e.tag, cua.CuaError_Tags.ImageNotPublished)
      assert.match(String(e.message ?? e), /not published yet/)
      return true
    })
  }
}

test("Image tiers pick <os-version>[-<tier>] and Image.omarchy() is edge", () => {
  const saved = process.env.CUA_IMAGE_LINUX
  delete process.env.CUA_IMAGE_LINUX
  try {
    assert.equal(cua.Image.linux(undefined, "full"), "ghcr.io/trycua/linux:24.04")
    assert.equal(cua.Image.linux({ tier: "full" }), "ghcr.io/trycua/linux:24.04")
    expectRef(() => cua.Image.linux({ tier: "slim" }), "ghcr.io/trycua/linux:24.04-slim")
    expectRef(() => cua.Image.macos(undefined, "xcode"), "ghcr.io/trycua/macos:26-xcode")
    expectRef(() => cua.Image.omarchy(), "ghcr.io/trycua/omarchy:edge")
    assert.throws(() => cua.Image.linux({ tier: "xcode" }))
    assert.throws(() => cua.Image.linux({ tier: "tiny" }))
  } finally {
    if (saved !== undefined) process.env.CUA_IMAGE_LINUX = saved
  }
})
