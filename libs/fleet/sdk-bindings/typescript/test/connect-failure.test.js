import assert from "node:assert/strict"
import { chmod, mkdtemp, readFile, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { test } from "node:test"
import { setTimeout as delay } from "node:timers/promises"

import { connect } from "../dist/index.js"

test("connect terminates the core runner when configuration fails", async () => {
  const directory = await mkdtemp(join(tmpdir(), "cyclops-sdk-connect-"))
  const runner = join(directory, "runner.mjs")
  const pidFile = join(directory, "pid")
  const terminatedFile = join(directory, "terminated")
  await writeFile(runner, `#!/usr/bin/env node
import { writeFileSync } from "node:fs"
import { createInterface } from "node:readline"
writeFileSync(${JSON.stringify(pidFile)}, String(process.pid))
process.on("SIGTERM", () => { writeFileSync(${JSON.stringify(terminatedFile)}, "yes"); process.exit(0) })
createInterface({ input: process.stdin }).once("line", () => process.stdout.write('{"ok":false,"error":"invalid configuration"}\\n'))
`)
  await chmod(runner, 0o755)
  const previousRunner = process.env.CYCLOPS_CORE_RUNNER
  process.env.CYCLOPS_CORE_RUNNER = runner
  let pid
  try {
    await assert.rejects(
      connect({
        baseUrl: "invalid",
        oauth: { tokenUrl: "https://auth.example/token", clientId: "id", clientSecret: "secret" },
      }),
      /invalid configuration/,
    )
    pid = Number(await readFile(pidFile, "utf8"))
    for (let attempt = 0; attempt < 20; attempt++) {
      try {
        assert.equal(await readFile(terminatedFile, "utf8"), "yes")
        return
      } catch {
        await delay(25)
      }
    }
    assert.fail("core runner was not terminated")
  } finally {
    if (previousRunner === undefined) delete process.env.CYCLOPS_CORE_RUNNER
    else process.env.CYCLOPS_CORE_RUNNER = previousRunner
    if (pid) {
      try { process.kill(pid, "SIGKILL") } catch {}
    }
  }
})
