// share-a-link (TypeScript). See ../python/test_share.py for the scenario;
// the assertions here are the same.
import assert from "node:assert/strict"
import { mkdtempSync, writeFileSync } from "node:fs"
import { createServer } from "node:http"
import { join } from "node:path"

import * as e from "./lib.mjs"

const { cua } = e

e.e2eTest("share-a-link", "hermetic", "files and ports become links a daemon owns", async () => {
  const app = createServer((req, res) => res.end(`app says ${req.url}`))
  await new Promise((r) => app.listen(0, "127.0.0.1", r))
  const appPort = app.address().port
  const dir = mkdtempSync("/tmp/cua-share-ts-")
  const report = join(dir, "report.html")
  writeFileSync(report, "<h1>report</h1>")
  try {
    await e.withShareDaemon(async (d) => {
      const c = d.client()
      const f = await c.share(report)
      assert.equal(f.access, cua.ShareAccess.Private)
      assert.equal(f.kind, "file")
      assert.ok(f.url.startsWith(`${d.relayUrl}/m/${e.SHARE_MACHINE}/share/`), f.url)
      assert.deepEqual(await d.get(f.url, "owner"), [200, "<h1>report</h1>"])
      assert.equal((await d.get(f.url))[0], 401)
      assert.equal((await d.get(f.url, "other"))[0], 403)
      const check = await c.checkShare(f.id)
      assert.ok(check.ok && check.status === 200 && check.via === "link", JSON.stringify(check))

      const p = await c.share(`localhost:${appPort}`, cua.ShareAccess.Private, undefined, "dash")
      assert.deepEqual(await d.get(`${p.url}x?y=1`, "owner"), [200, "app says /x?y=1"])
      assert.equal((await c.shares()).length, 2)
      assert.deepEqual(await c.stopShare("dash"), [p.id])
      assert.equal((await d.get(p.url, "owner"))[0], 404)

      const pub = await c.share(dir, cua.ShareAccess.Public, 60)
      assert.ok(pub.url.includes("/share/pub/") && pub.expiresAtUnix != null)
      const [code, body] = await d.get(pub.url)
      assert.ok(code === 200 && body.includes("report.html"), body)

      assert.equal((await d.client().stopAllShares()).length, 2)
      assert.equal((await d.get(pub.url))[0], 404)
      assert.equal((await c.shares()).length, 0)

      await assert.rejects(c.share(join(dir, "missing")), (err) => e.isErr(err, "InvalidArgument"))
      await assert.rejects(c.share("0"), (err) => e.isErr(err, "InvalidArgument"))
      await assert.rejects(c.share(report, cua.ShareAccess.Public, 5), (err) => e.isErr(err, "InvalidArgument"))
      await assert.rejects(c.stopShare("share-nope"), (err) => e.isErr(err, "NotFound"))

      const dead = await c.share("1")
      const bad = await c.checkShare(dead.id)
      assert.ok(!bad.ok && bad.status === 502, JSON.stringify(bad))
      await c.stopAllShares()
    })
  } finally {
    app.close()
  }
}, { timeout: 120_000 })
