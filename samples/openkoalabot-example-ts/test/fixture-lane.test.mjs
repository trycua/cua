// The whole shared scenario against `cua-test-fixtures` (a real
// cua-spacesd core on loopback, confined to temp HOME/PATH). Skipped when
// the fixture binary or the native library is missing.
import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { existsSync, readFileSync } from "node:fs"
import { createInterface } from "node:readline"
import { fileURLToPath } from "node:url"
import { test } from "node:test"

const fixtures = process.env.CUA_TEST_FIXTURES ?? fileURLToPath(new URL("../../../libs/cua/target/debug/cua-test-fixtures", import.meta.url))
const specPath = fileURLToPath(new URL("../../openkoalabot-example-scenario/scenario.json", import.meta.url))

test("scenario on the fixture lane", { skip: !existsSync(fixtures) && "cua-test-fixtures is not built", timeout: 300_000 }, async () => {
  const { runScenario } = await import("../dist/scenario/runner.js")
  const { parseScenario } = await import("../dist/core/spec.js")
  const child = spawn(fixtures, [], { stdio: ["pipe", "pipe", "ignore"] })
  try {
    const fx = JSON.parse(await new Promise((r, j) => {
      createInterface({ input: child.stdout }).once("line", r)
      child.once("exit", () => j(new Error("fixtures exited")))
    }))
    const result = await runScenario({
      spec: parseScenario(readFileSync(specPath, "utf8")),
      specDir: fileURLToPath(new URL("../../openkoalabot-example-scenario/", import.meta.url)),
      lane: "fixture",
      url: fx.spaces_url,
      token: fx.spaces_token,
      importRoot: fx.spaces_teleport_home,
    })
    const by = Object.fromEntries(result.steps.map((s) => [s.id, s]))
    assert.equal(result.ok, true, JSON.stringify(result.steps, null, 2))
    for (const id of ["space", "file", "delete"]) assert.equal(by[id].status, "pass", `${id}: ${by[id].detail}`)
    // The runner embeds the MIT runtime, which refuses session teleport
    // (HostCapabilityMissing): teleport ships with Cua Spaces.
    assert.equal(by.teleport.status, "skip", by.teleport.detail)
    assert.match(by.teleport.detail, /ships with Cua Spaces/)
    // The scenario's fake agent CLI predates ACP agent runs (real harness
    // runs: cua-agents' e2e_live). No desktop and no presence service in the
    // fixture driver.
    assert.equal(by.agent.status, "skip")
    assert.equal(by.stream.status, "skip")
    assert.equal(by.presence.status, "skip")
  } finally {
    child.stdin.end()
  }
})
