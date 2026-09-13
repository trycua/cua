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
      contract_version: "0.8.0",
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
      assert.deepEqual(JSON.parse(await driver.listToolsJson()), {
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
    const requests = []
    const requestsPromise = readyPromise.then(
      () =>
        new Promise((resolve, reject) => {
          fixture.on("error", reject)
          fixture.on("message", (message) => {
            if (message.request) requests.push(message.request)
            if (requests.length === 8) resolve(requests)
          })
        }),
    )

    try {
      await readyPromise
      assert.equal(existsSync(socketPath), true)
      const sdk = await import("@trycua/cua-driver")
      const {
        ActionEffect,
        ActionRoute,
        ActionTarget,
        ClickPosition,
        InputDeliveryMode,
        ClickButton,
        ClickInput,
        CuaDriver,
        StatePredicate,
        VerificationStatus,
        VerifyStateInput,
        WindowPredicate,
      } = sdk
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
        "getSession",
        "listSessions",
        "getSessionState",
        "endSession",
        "getDesktopState",
        "listApps",
        "listWindows",
        "getWindowState",
        "getScreenSize",
        "getCursorPosition",
        "moveCursor",
        "click",
        "drag",
        "scroll",
        "typeText",
        "pressKey",
        "hotkey",
        "verifyState",
      ]
      assert.equal(
        expectedMethods.every((name) => typeof driver[name] === "function"),
        true,
      )
      const verificationResult = await driver.verifyState(
        VerifyStateInput.new({
          pid: 123n,
          windowId: 456n,
          expect: [
            StatePredicate.new({
              window: WindowPredicate.new({ exists: true }),
            }),
          ],
          session: "node-run",
          timeoutMs: 0n,
          stableSamples: 1n,
          includeScreenshot: true,
        }),
      )
      const actionResult = await driver.click(
        ClickInput.new({
          position: new ClickPosition.Coordinates({ x: 12, y: 34 }),
          target: new ActionTarget.Desktop({ displayId: "primary" }),
          deliveryMode: InputDeliveryMode.Foreground,
          session: "node-run",
          button: ClickButton.Left,
          count: 1,
        }),
      )
      const apps = await driver.listApps(sdk.ListAppsInput.new({}))
      assert.equal(apps.apps[0].name, "Editor")
      const windows = await driver.listWindows(sdk.ListWindowsInput.new({ pid: 42, onScreenOnly: true }))
      assert.equal(windows.windows[0].windowId, 123n)
      assert.equal(windows.windows[0].zIndex, undefined)
      const state = await driver.getWindowState(sdk.GetWindowStateInput.new({
        pid: 42, windowId: 123n, session: "node-run", query: "Save",
        includeScreenshot: true, includeAccessibilityTree: true,
        maxElements: 10, maxDepth: 3, maxDimension: 800,
      }))
      assert.equal(state.snapshotId, "snapshot-1")
      assert.equal(state.elements[0].label, "Save")
      assert.equal(state.images[0].mimeType, "image/png")
      assert.equal(state.images[0].dataBase64, "cG5n")
      const tokenClick = (token, windowId) => ClickInput.new({
        target: new ActionTarget.Window({ pid: 42, windowId }),
        position: new ClickPosition.Element({ elementToken: token }),
        deliveryMode: InputDeliveryMode.Background, session: "node-run",
      })
      const freshAction = await driver.click(tokenClick(state.elements[0].elementToken, 123n))
      assert.equal(freshAction.effect, ActionEffect.Unverifiable)
      // Service fixtures qualify SDK error propagation, not native token validation.
      for (const [token, windowId, code] of [
        ["stale-token", 123n, "stale_element_token"],
        ["fresh-token", 124n, "element_target_mismatch"],
      ]) {
        await assert.rejects(driver.click(tokenClick(token, windowId)), error => {
          assert.equal(sdk.DriverError.Tool.instanceOf(error), true)
          assert.equal(error.inner.tool, "click")
          assert.equal(error.inner.errorCode, code)
          return true
        })
      }
      await requestsPromise
      driver.uniffiDestroy()

      assert.equal(verificationResult.text, "node ffi")
      assert.equal(verificationResult.images[0].mimeType, "image/png")
      assert.equal(verificationResult.action, undefined)
      assert.equal(verificationResult.verification.status, VerificationStatus.Satisfied)
      assert.equal(actionResult.verification, undefined)
      assert.equal(actionResult.effect, ActionEffect.Unverifiable)
      assert.equal(actionResult.route, ActionRoute.GlobalInput)
      assert.equal("verified" in actionResult, false)
      assert.deepEqual(requests[2].args, {})
      assert.deepEqual(requests[3].args, { pid: 42, on_screen_only: true })
      assert.deepEqual(requests[4].args, {
        pid: 42, window_id: 123, session: "node-run", query: "Save",
        include_screenshot: true, include_accessibility_tree: true,
        max_elements: 10, max_depth: 3, max_dimension: 800,
      })
      assert.deepEqual(requests[5].args, {
        target: { kind: "window", pid: 42, window_id: 123 },
        element_token: "fresh-token", delivery_mode: "background", session: "node-run",
      })
      assert.equal(requests[6].args.element_token, "stale-token")
      assert.equal(requests[7].args.target.window_id, 124)
      assert.equal(requests[0].name, "verify_state")
      assert.deepEqual(requests[0].args, {
        pid: 123,
        window_id: 456,
        expect: [{ window: { exists: true } }],
        session: "node-run",
        timeout_ms: 0,
        stable_samples: 1,
        include_screenshot: true,
      })
      assert.equal(requests[0].client_kind, "typescript_sdk")
      assert.equal(requests[1].name, "click")
      assert.deepEqual(requests[1].args, {
        x: 12,
        y: 34,
        target: { kind: "desktop", display_id: "primary" },
        delivery_mode: "foreground",
        session: "node-run",
        button: "left",
        count: 1,
      })
    } finally {
      fixture.kill()
      rmSync(directory, { recursive: true, force: true })
    }
  },
)

test(
  "generated Node SDK can own the runtime in process",
  { skip: !existsSync(library), timeout: 10_000 },
  async () => {
    const { CuaDriver, DriverExecutionMode } = await import("@trycua/cua-driver")
    const driver = CuaDriver.create(undefined)
    try {
      assert.equal(driver.executionMode(), DriverExecutionMode.Embedded)
      assert.equal(driver.socketPath(), "")
      assert.equal(driver.isAvailable(), true)
      const metadata = await driver.metadata()
      assert.equal(metadata.embedded, true)
      assert.equal(metadata.pid, process.pid)
      await driver.shutdown()
      assert.equal(driver.isAvailable(), false)
    } finally {
      driver.uniffiDestroy()
    }
  },
)
