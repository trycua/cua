// Space lifetimes, streamed downloads and tunnel forwards through the Node
// binding, against the fixture's in-process cua-spacesd core (temp guest
// HOME; the "guest" port is a loopback socket this test opens). No VMs.
import assert from "node:assert/strict"
import { randomBytes } from "node:crypto"
import { mkdtempSync } from "node:fs"
import { createConnection, createServer } from "node:net"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { after, before, test } from "node:test"

import * as cua from "../dist/index.js"
import { binary, startFixtures } from "./fixtures.mjs"

let fx
before(async () => {
  fx = await startFixtures()
})
after(async () => {
  await fx?.stop()
})

function echoServer() {
  return new Promise((resolve) => {
    const srv = createServer((sock) => sock.on("data", (d) => sock.end(d)))
    srv.listen(0, "127.0.0.1", () => resolve(srv))
  })
}

test("lifetimes, streamed downloads and forwards", { skip: !binary("CUA_TEST_FIXTURES", "cua-test-fixtures") }, async () => {
  const dir = mkdtempSync(join(tmpdir(), "cua-ts-life-"))
  const c = cua.embedded({ stateDir: join(dir, "sbx"), spacesHome: join(dir, "cua") })
  const spaces = c.spaces()
  const info = await spaces.add(fx.spaces_url, fx.spaces_token, "ts-lifetimes")

  const now = Date.now() / 1000
  const at = Number(await spaces.setLifetime(info.id, 60n, undefined))
  assert.ok(at > now + 55 && at <= now + 62, String(at))
  await assert.rejects(spaces.setLifetime(info.id, 1n, "explode"))

  const env = (await spaces.space(info.id)).spacesd()
  const guest = join(fx.spaces_guest_home, "ts", "stream.bin")
  const blob = randomBytes(3 * 1024 * 1024 + 5)
  await env.upload(guest, blob.buffer.slice(blob.byteOffset, blob.byteOffset + blob.length), undefined)
  const d = await env.openDownload(guest, undefined)
  const parts = []
  for (let c_; (c_ = await d.nextChunk()) !== undefined; ) parts.push(Buffer.from(c_))
  assert.ok(parts.length > 1)
  assert.ok(Buffer.concat(parts).equals(blob))

  const srv = await echoServer()
  const port = srv.address().port
  const fwd = await env.forwardTcp(port, undefined)
  assert.equal(fwd.guestPort(), port)
  const [host, p] = fwd.localAddr().split(":")
  const echoed = await new Promise((resolve, reject) => {
    const s = createConnection({ host, port: Number(p) }, () => s.write("ping"))
    s.on("data", (b) => resolve(b.toString()))
    s.on("error", reject)
  })
  assert.equal(echoed, "ping")
  await fwd.stop()
  srv.close()

  await spaces.delete_(info.id)
})
