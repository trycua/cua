// Spaces.build: the options and result records, and the checks that run
// before any Space starts (the build itself is cua-spaces' QEMU test and
// the Mac run in the PR).
import assert from "node:assert/strict"
import { mkdtempSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"

import * as cua from "../dist/index.js"

function embedded() {
  const dir = mkdtempSync(join(tmpdir(), "cua-ts-build-"))
  const c = cua.embedded({
    stateDir: join(dir, "sbx"),
    spacesHome: join(dir, "cua"),
  })
  return { dir, spaces: c.spaces() }
}

test("BuildOptions defaults and steps", () => {
  const o = cua.BuildOptions.create({ base: "ubuntu:24.04" })
  assert.equal(o.dockerfile, undefined)
  assert.equal(o.tag, undefined)
  assert.deepEqual(o.steps, [])
  assert.equal(o.noCache, false)
  assert.equal(o.buildArgs.size, 0)
  const run = new cua.BuildStep.Run({ command: "make" })
  assert.equal(run.tag, cua.BuildStep_Tags.Run)
  assert.equal(run.inner.command, "make")
  const copy = new cua.BuildStep.Copy({
    sources: ["a.txt"],
    dest: "/srv/",
    owner: undefined,
    mode: "640",
    unpack: false,
  })
  assert.equal(copy.inner.mode, "640")
  // On a create: build is part of the options.
  const create = cua.SpaceCreateOptions.create({ build: o })
  assert.equal(create.build.base, "ubuntu:24.04")
  assert.equal(cua.SpaceCreateOptions.create({}).build, undefined)
})

test("a build is refused before anything runs when its input is wrong", async () => {
  const { dir, spaces } = embedded()
  // Neither a Dockerfile nor a base.
  await assert.rejects(spaces.build(cua.BuildOptions.create({})), (e) =>
    cua.CuaError.InvalidArgument.instanceOf(e),
  )
  // Multi-stage names the line.
  await assert.rejects(
    spaces.build(cua.BuildOptions.create({ dockerfile: "FROM linux\nRUN true\nFROM linux\n" })),
    (e) => cua.CuaError.InvalidArgument.instanceOf(e) && /line 3/.test(String(e.message ?? e)),
  )
  // A COPY of a file the context does not have.
  writeFileSync(join(dir, "Dockerfile"), "FROM linux\nCOPY missing.txt /x\n")
  await assert.rejects(
    spaces.build(
      cua.BuildOptions.create({
        dockerfile: "FROM linux\nCOPY missing.txt /x\n",
        contextDir: dir,
      }),
    ),
    (e) => cua.CuaError.InvalidArgument.instanceOf(e) && /missing\.txt/.test(String(e.message ?? e)),
  )
  // A base that cannot run as a Space says what works.
  await assert.rejects(
    spaces.build(cua.BuildOptions.create({ base: "alpine:3" })),
    (e) => cua.CuaError.InvalidArgument.instanceOf(e) && /ubuntu/.test(String(e.message ?? e)),
  )
  // Builds stay on the machine that took the checkpoints.
  await assert.rejects(
    spaces.build(cua.BuildOptions.create({ base: "linux", on: "cloud" })),
    (e) => /not supported/.test(String(e.message ?? e)),
  )
  assert.deepEqual(await spaces.builds(), [])
  await assert.rejects(spaces.deleteBuild("nope"), (e) => cua.CuaError.NotFound.instanceOf(e))
})
