import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { existsSync, readFileSync } from "node:fs"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const testDirectory = path.dirname(fileURLToPath(import.meta.url))
const electron = path.resolve(testDirectory, "../node_modules/.bin/electron")
const packageVersion = JSON.parse(
  readFileSync(path.resolve(testDirectory, "../package.json"), "utf8"),
).version

test(
  "Electron main receives embedded connection values and cleans up lifecycle",
  { skip: process.platform === "win32" || !existsSync(electron), timeout: 60_000 },
  async () => {
    const command = process.platform === "linux" ? "xvfb-run" : electron
    const args = process.platform === "linux"
      ? ["-a", electron, "--no-sandbox", path.join(testDirectory, "electron-main-fixture.mjs")]
      : [path.join(testDirectory, "electron-main-fixture.mjs")]
    const child = spawn(command, args, { stdio: ["ignore", "pipe", "pipe"] })
    let stdout = ""
    let stderr = ""
    child.stdout.setEncoding("utf8").on("data", (chunk) => { stdout += chunk })
    child.stderr.setEncoding("utf8").on("data", (chunk) => { stderr += chunk })
    const code = await new Promise((resolve, reject) => {
      child.on("error", reject)
      child.on("exit", resolve)
    })

    assert.equal(code, 0, `stdout:\n${stdout}\nstderr:\n${stderr}`)
    const result = JSON.parse(stdout.trim().split("\n").at(-1))
    assert.equal(result.versions.electron, "43.2.0")
    assert.equal(result.versions.package, packageVersion)
    assert.equal(result.connection.driverVersion, packageVersion)
    assert.equal(result.connection.socketPathIsPrivate, true)
    assert.equal(result.connection.pidMatchesChild, true)
    assert.equal(result.connection.stateIsReady, true)
    assert.equal(result.connection.mcpCommand.endsWith("generic-driver-host"), true)
    assert.equal(result.connection.mcpArgs.includes("mcp"), true)
    assert.equal(result.connection.mcpArgs.includes("--embedded"), true)
    assert.deepEqual(result.connection.tools, { tools: [{ name: "electron_fixture" }] })
    assert.deepEqual(result.cleanup, { socketRemoved: true, childExited: true })
    assert.deepEqual(result.failureCleanup, {
      rejected: true,
      stateIsStopped: true,
      socketRemoved: true,
      childExited: true,
    })
  },
)
