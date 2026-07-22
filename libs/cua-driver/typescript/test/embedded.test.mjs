import assert from "node:assert/strict"
import { chmod, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises"
import { createServer } from "node:net"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { afterEach, describe, test } from "node:test"

import { EmbeddedCuaDriver } from "../dist/embedded.js"

const drivers = []
const temporaryDirectories = []

afterEach(async () => {
  await Promise.all(drivers.splice(0).map(driver => driver.stop()))
  await Promise.all(
    temporaryDirectories.splice(0).map(directory =>
      rm(directory, { recursive: true, force: true }),
    ),
  )
})

async function makeFakeDriver() {
  const directory = await mkdtemp(join(tmpdir(), "cua embedded test "))
  temporaryDirectories.push(directory)
  const binaryPath = join(directory, "fake cua-driver")
  const journalPath = join(directory, "argv.json")
  await writeFile(
    binaryPath,
    `#!/usr/bin/env node
const fs = require("node:fs");
const net = require("node:net");
const args = process.argv.slice(2);
fs.writeFileSync(${JSON.stringify(journalPath)}, JSON.stringify(args));
const socketIndex = args.indexOf("--socket");
const socketPath = args[socketIndex + 1];
const server = net.createServer((socket) => socket.end());
server.listen(socketPath);
const stop = () => server.close(() => {
  if (process.platform !== "win32") try { fs.unlinkSync(socketPath); } catch {}
  process.exit(0);
});
process.on("SIGTERM", stop);
process.on("SIGINT", stop);
`,
  )
  await chmod(binaryPath, 0o755)
  return { binaryPath, journalPath }
}

function makeDriver(binaryPath, socketPath) {
  const driver = new EmbeddedCuaDriver({
    binaryPath,
    hostBundleId: "com.example.host",
    socketPath,
    startupTimeoutMs: 2_000,
    shutdownTimeoutMs: 1_000,
    stderr: "ignore",
  })
  drivers.push(driver)
  return driver
}

describe("embedded cua-driver lifecycle", { skip: process.platform === "win32" }, () => {
  test("starts a direct private daemon and returns SDK and MCP connection details", async () => {
    const { binaryPath, journalPath } = await makeFakeDriver()
    const socketPath = join(tmpdir(), `cua driver ${process.pid}.sock`)
    const driver = makeDriver(binaryPath, socketPath)

    const [first, second] = await Promise.all([driver.start(), driver.start()])

    assert.deepEqual(second, first)
    assert.equal(first.socketPath, socketPath)
    assert.deepEqual(first.mcp, {
      command: binaryPath,
      args: ["mcp", "--embedded", "--socket", socketPath],
      env: {
        CUA_DRIVER_EMBEDDED: "1",
        CUA_DRIVER_HOST_BUNDLE_ID: "com.example.host",
      },
    })
    assert.deepEqual(JSON.parse(await readFile(journalPath, "utf8")), [
      "serve",
      "--embedded",
      "--socket",
      socketPath,
      "--host-bundle-id",
      "com.example.host",
    ])
    assert.equal((await stat(socketPath)).mode & 0o777, 0o600)

    await driver.stop()
    await assert.rejects(stat(socketPath))
  })

  test("keeps its default unix socket below the sockaddr length limit", async () => {
    const { binaryPath } = await makeFakeDriver()
    const driver = new EmbeddedCuaDriver({
      binaryPath,
      hostBundleId: "com.example.host",
      startupTimeoutMs: 2_000,
      stderr: "ignore",
    })
    drivers.push(driver)

    const connection = await driver.start()
    assert.ok(Buffer.byteLength(connection.socketPath) < 104)
  })

  test("restarts with a new child and cleans up on repeated stop", async () => {
    const { binaryPath } = await makeFakeDriver()
    const socketPath = join(tmpdir(), `cua-driver-restart-${process.pid}.sock`)
    const driver = makeDriver(binaryPath, socketPath)
    const first = await driver.start()
    const second = await driver.restart()

    assert.notEqual(second.pid, first.pid)
    await driver.stop()
    await driver.stop()
    await assert.rejects(stat(socketPath))
  })

  test("recovers after the daemon exits unexpectedly", async () => {
    const { binaryPath } = await makeFakeDriver()
    const socketPath = join(tmpdir(), `cua-driver-crash-${process.pid}.sock`)
    const driver = makeDriver(binaryPath, socketPath)
    const first = await driver.start()

    process.kill(first.pid, "SIGTERM")
    for (let attempt = 0; attempt < 100 && driver.connection; attempt += 1) {
      await new Promise(resolve => setTimeout(resolve, 10))
    }
    assert.equal(driver.connection, undefined)

    const second = await driver.start()
    assert.notEqual(second.pid, first.pid)
  })

  test("reports an early exit and stops polling the socket", async () => {
    const socketPath = join(tmpdir(), `cua-driver-exit-${process.pid}.sock`)
    const driver = makeDriver("/usr/bin/true", socketPath)

    await assert.rejects(driver.start(), { code: "exited-before-ready" })

    let connections = 0
    const server = createServer(socket => {
      connections += 1
      socket.end()
    })
    await new Promise((resolve, reject) => {
      server.once("error", reject)
      server.listen(socketPath, resolve)
    })
    await new Promise(resolve => setTimeout(resolve, 100))
    await new Promise(resolve => server.close(resolve))
    assert.equal(connections, 0)
  })

  test("times out when the daemon never creates its socket", async () => {
    const directory = await mkdtemp(join(tmpdir(), "cua embedded timeout "))
    temporaryDirectories.push(directory)
    const binaryPath = join(directory, "unready-driver")
    await writeFile(binaryPath, "#!/usr/bin/env node\nsetInterval(() => {}, 1000);\n")
    await chmod(binaryPath, 0o755)
    const driver = new EmbeddedCuaDriver({
      binaryPath,
      hostBundleId: "com.example.host",
      startupTimeoutMs: 100,
      shutdownTimeoutMs: 100,
      stderr: "ignore",
    })
    drivers.push(driver)

    await assert.rejects(driver.start(), { code: "startup-timeout" })
  })

  test("force-kills an unresponsive child after startup failure", async () => {
    const directory = await mkdtemp(join(tmpdir(), "cua-embedded-unresponsive-"))
    temporaryDirectories.push(directory)
    const binaryPath = join(directory, "unresponsive-driver")
    const pidPath = join(directory, "pid")
    await writeFile(
      binaryPath,
      `#!/usr/bin/env node
const fs = require("node:fs");
fs.writeFileSync(${JSON.stringify(pidPath)}, String(process.pid));
process.on("SIGTERM", () => process.stderr.write("ignored SIGTERM\\n"));
setInterval(() => {}, 1000);
`,
    )
    await chmod(binaryPath, 0o755)
    const driver = new EmbeddedCuaDriver({
      binaryPath,
      hostBundleId: "com.example.host",
      startupTimeoutMs: 1_000,
      shutdownTimeoutMs: 100,
      stderr: "ignore",
    })
    drivers.push(driver)

    await assert.rejects(driver.start(), { code: "startup-timeout" })
    const pid = Number(await readFile(pidPath, "utf8"))
    assert.throws(() => process.kill(pid, 0))
  })

  test("cancels startup before the child is spawned", async () => {
    const { binaryPath } = await makeFakeDriver()
    const socketPath = join(tmpdir(), `cua-driver-immediate-stop-${process.pid}.sock`)
    const driver = makeDriver(binaryPath, socketPath)

    const starting = driver.start()
    await driver.stop()

    await assert.rejects(starting, { code: "startup-cancelled" })
    assert.equal(driver.connection, undefined)
    await assert.rejects(stat(socketPath))
  })

  test("cancels startup while waiting for the socket", async () => {
    const directory = await mkdtemp(join(tmpdir(), "cua embedded stop "))
    temporaryDirectories.push(directory)
    const binaryPath = join(directory, "unready-driver")
    await writeFile(binaryPath, "#!/usr/bin/env node\nsetInterval(() => {}, 1000);\n")
    await chmod(binaryPath, 0o755)
    const driver = new EmbeddedCuaDriver({
      binaryPath,
      hostBundleId: "com.example.host",
      startupTimeoutMs: 10_000,
      shutdownTimeoutMs: 100,
      stderr: "ignore",
    })
    drivers.push(driver)

    const starting = driver.start()
    await new Promise(resolve => setTimeout(resolve, 20))
    const stopping = driver.stop()

    await assert.rejects(starting, { code: "startup-cancelled" })
    await stopping
  })
})
