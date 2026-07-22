import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import { chmodSync, existsSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"
import test from "node:test"
import { fileURLToPath } from "node:url"

const testDirectory = path.dirname(fileURLToPath(import.meta.url))
const libraryName =
  process.platform === "darwin"
    ? "libcua_driver_sdk.dylib"
    : process.platform === "win32"
      ? "cua_driver_sdk.dll"
      : "libcua_driver_sdk.so"
const library = path.resolve(testDirectory, "../dist/native", libraryName)

if (process.env.CUA_DRIVER_REQUIRE_UNIFFI === "1" && !existsSync(library)) {
  throw new Error(`required staged UniFFI library is missing: ${library}`)
}

test(
  "embedded host supplies the private socket to the Rust SDK",
  { skip: process.platform === "win32" || !existsSync(library), timeout: 10_000 },
  async () => {
    const directory = mkdtempSync(path.join(os.tmpdir(), "cua-driver-embedded-sdk-"))
    const binaryPath = path.join(directory, "fake cua-driver")
    writeFileSync(
      binaryPath,
      `#!/usr/bin/env node
const net = require("node:net");
const args = process.argv.slice(2);
const socketPath = args[args.indexOf("--socket") + 1];
const server = net.createServer(socket => socket.end());
server.listen(socketPath);
process.on("SIGTERM", () => server.close(() => process.exit(0)));
`,
    )
    chmodSync(binaryPath, 0o755)

    const { EmbeddedCuaDriver } = await import("@trycua/cua-driver/embedded")
    const embedded = new EmbeddedCuaDriver({
      binaryPath,
      hostBundleId: "com.example.t3",
      socketPath: path.join(directory, "driver.sock"),
      stderr: "ignore",
    })

    try {
      const connection = await embedded.start()
      const sdk = await import("@trycua/cua-driver")
      const driver = sdk.CuaDriver.connect(connection.socketPath)
      assert.equal(driver.socketPath(), connection.socketPath)
      driver.uniffiDestroy()
    } finally {
      await embedded.stop()
      rmSync(directory, { recursive: true, force: true })
    }
  },
)

test(
  "generated Node SDK bindings call the Rust daemon interface",
  { skip: process.platform === "win32" || !existsSync(library), timeout: 10_000 },
  async () => {
    const directory = mkdtempSync(path.join(os.tmpdir(), "cua-driver-node-ffi-"))
    const socketPath = path.join(directory, "driver.sock")
    const fixture = spawn(
      process.execPath,
      [path.join(testDirectory, "native-daemon-fixture.mjs"), socketPath],
      { stdio: ["ignore", "inherit", "inherit", "ipc"] },
    )
    const readyPromise = new Promise((resolve, reject) => {
      fixture.on("error", reject)
      fixture.on("message", (message) => {
        if (message.ready) resolve(null)
      })
    })
    const requestPromise = readyPromise.then(
      () =>
        new Promise((resolve, reject) => {
          fixture.on("error", reject)
          fixture.on("message", (message) => {
            if (message.request) resolve(message.request)
          })
        }),
    )

    try {
      await readyPromise
      assert.equal(existsSync(socketPath), true)
      const sdk = await import("@trycua/cua-driver")
      const { CuaDriver, GetDesktopStateInput } = sdk
      assert.equal("StdioMcpTransport" in sdk, false)
      await assert.rejects(
        import("@trycua/cua-driver/sdk"),
        error => error?.code === "ERR_PACKAGE_PATH_NOT_EXPORTED",
      )
      await assert.rejects(
        import("@trycua/cua-driver/native"),
        error => error?.code === "ERR_PACKAGE_PATH_NOT_EXPORTED",
      )
      const driver = CuaDriver.connect(socketPath)
      const expectedMethods = [
        "startSession",
        "escalateSession",
        "getSessionState",
        "endSession",
        "getDesktopState",
        "getScreenSize",
        "getCursorPosition",
        "moveCursor",
        "click",
        "drag",
        "scroll",
        "typeText",
        "pressKey",
        "hotkey",
      ]
      assert.equal(
        expectedMethods.every((name) => typeof driver[name] === "function"),
        true,
      )
      const result = driver.getDesktopState(
        GetDesktopStateInput.new({ session: "node-run" }),
      )
      const request = await requestPromise
      driver.uniffiDestroy()

      assert.equal(result.text, "node ffi")
      assert.equal(result.images[0].mimeType, "image/png")
      assert.equal(result.verified, true)
      assert.equal(request.name, "get_desktop_state")
      assert.deepEqual(request.args, { session: "node-run" })
    } finally {
      fixture.kill()
      rmSync(directory, { recursive: true, force: true })
    }
  },
)
