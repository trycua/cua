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
const nodeTriple =
  process.platform === "darwin"
    ? `darwin-${process.arch}`
    : process.platform === "win32"
      ? `win32-${process.arch}-msvc`
      : `linux-${process.arch}-${process.report.getReport().header.glibcVersionRuntime ? "gnu" : "musl"}`
const library = path.resolve(
  testDirectory,
  "../node_modules/@trycua",
  `cua-driver-${nodeTriple}`,
  libraryName,
)

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
const fs = require("node:fs");
const args = process.argv.slice(2);
const socketPath = args[args.indexOf("--socket") + 1];
const hostBundleId = args[args.indexOf("--host-bundle-id") + 1];
const server = net.createServer(socket => {
  let buffer = "";
  socket.setEncoding("utf8");
  socket.on("data", chunk => {
    buffer += chunk;
    if (!buffer.includes("\\n")) return;
    const request = JSON.parse(buffer.split("\\n", 1)[0]);
    const result = request.method === "metadata" ? {
      driver_version: "0.10.0",
      contract_version: "0.2.0",
      tools_list_schema_version: "1",
      capability_version: "1",
      mcp_protocol_version: "2025-06-18",
      pid: process.pid,
      embedded: true,
      host_bundle_id: hostBundleId,
    } : { tools: [{ name: "embedded_fixture" }] };
    socket.end(JSON.stringify({ ok: true, result }) + "\\n");
  });
});
server.listen(socketPath, () => fs.chmodSync(socketPath, 0o600));
process.stdin.resume();
process.stdin.on("end", () => server.close(() => process.exit(0)));
`,
    )
    chmodSync(binaryPath, 0o755)

    const { EmbeddedCuaDriverHost } = await import("@trycua/cua-driver/embedded")
    const embedded = new EmbeddedCuaDriverHost(binaryPath, "com.example.t3")

    try {
      const connection = await embedded.start()
      const sdk = await import("@trycua/cua-driver")
      const driver = sdk.CuaDriver.connect(connection.socketPath)
      assert.equal(driver.socketPath(), connection.socketPath)
      assert.deepEqual(JSON.parse(driver.listToolsJson()), {
        tools: [{ name: "embedded_fixture" }],
      })
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
