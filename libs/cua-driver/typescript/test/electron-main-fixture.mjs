import { chmodSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import os from "node:os"
import path from "node:path"

const directory = mkdtempSync(path.join(os.tmpdir(), "cua-driver-electron-host-"))
const binaryPath = path.join(directory, "generic-driver-host")
const packageVersion = JSON.parse(
  readFileSync(new URL("../package.json", import.meta.url), "utf8"),
).version

writeFileSync(
  binaryPath,
  `#!/usr/bin/env node
const fs = require("node:fs");
const net = require("node:net");
const path = require("node:path");
const args = process.argv.slice(2);
const socketPath = args[args.indexOf("--socket") + 1];
const hostBundleId = args[args.indexOf("--host-bundle-id") + 1];
fs.writeFileSync(path.join(path.dirname(process.argv[1]), hostBundleId + ".pid"), String(process.pid));
const server = net.createServer(socket => {
  let buffer = "";
  socket.setEncoding("utf8");
  socket.on("data", chunk => {
    buffer += chunk;
    if (!buffer.includes("\\n")) return;
    const request = JSON.parse(buffer.split("\\n", 1)[0]);
    const result = request.method === "metadata" ? {
      driver_version: ${JSON.stringify(packageVersion)},
      contract_version: hostBundleId.endsWith(".failure") ? "incompatible" : "0.2.0",
      tools_list_schema_version: "1",
      capability_version: "1",
      mcp_protocol_version: "2025-06-18",
      pid: process.pid,
      embedded: true,
      host_bundle_id: hostBundleId,
    } : { tools: [{ name: "electron_fixture" }] };
    socket.end(JSON.stringify({ ok: true, result }) + "\\n");
  });
});
server.listen(socketPath, () => fs.chmodSync(socketPath, 0o600));
process.stdin.resume();
process.stdin.on("end", () => server.close(() => process.exit(0)));
`,
)
chmodSync(binaryPath, 0o755)

const processExists = (pid) => {
  try {
    process.kill(pid, 0)
    return true
  } catch (error) {
    if (error?.code === "ESRCH") return false
    throw error
  }
}

const waitForExit = async (pid) => {
  for (let attempt = 0; attempt < 40; attempt += 1) {
    if (!processExists(pid)) return true
    await new Promise((resolve) => setTimeout(resolve, 25))
  }
  return !processExists(pid)
}

try {
  const { EmbeddedCuaDriverHost, EmbeddedDriverHostState } = await import(
    "@trycua/cua-driver/embedded"
  )
  const host = new EmbeddedCuaDriverHost(binaryPath, "com.example.electron-host")
  const connection = await host.start()
  const socketPath = connection.socketPath
  const pid = connection.pid
  const { CuaDriver } = await import("@trycua/cua-driver")
  const driver = CuaDriver.connect(socketPath)
  const tools = JSON.parse(await driver.listToolsJson())
  driver.uniffiDestroy()
  const result = {
    versions: {
      electron: process.versions.electron,
      node: process.versions.node,
      package: packageVersion,
    },
    connection: {
      driverVersion: connection.driverVersion,
      socketPathIsPrivate:
        socketPath.startsWith(os.tmpdir()) && path.basename(socketPath).startsWith("cua-"),
      pidMatchesChild: Number(readFileSync(path.join(directory, "com.example.electron-host.pid"), "utf8")) === pid,
      stateIsReady: host.state() === EmbeddedDriverHostState.Ready,
      mcpCommand: connection.mcp.command,
      mcpArgs: connection.mcp.args,
      tools,
    },
  }
  await host.stop()
  host.uniffiDestroy()
  result.cleanup = {
    socketRemoved: !existsSync(socketPath),
    childExited: await waitForExit(pid),
  }

  const failingHost = new EmbeddedCuaDriverHost(binaryPath, "com.example.electron-host.failure")
  let failureCode
  try {
    await failingHost.start()
  } catch (error) {
    failureCode = error?.inner?.reason ?? error?.message
  }
  const failurePid = Number(
    readFileSync(path.join(directory, "com.example.electron-host.failure.pid"), "utf8"),
  )
  const failedSocket = failingHost.connection()?.socketPath
  result.failureCleanup = {
    rejected: typeof failureCode === "string" && failureCode.length > 0,
    stateIsStopped: failingHost.state() === EmbeddedDriverHostState.Stopped,
    socketRemoved: failedSocket === undefined || !existsSync(failedSocket),
    childExited: await waitForExit(failurePid),
  }
  await failingHost.stop()
  failingHost.uniffiDestroy()
  process.stdout.write(`${JSON.stringify(result)}\n`)
} catch (error) {
  process.stderr.write(`${error?.stack ?? error}\n`)
  process.exitCode = 1
} finally {
  rmSync(directory, { recursive: true, force: true })
}

process.exit(process.exitCode ?? 0)
