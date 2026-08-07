import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";

const HERE = dirname(fileURLToPath(import.meta.url));
const EXPECTED = JSON.parse(
  readFileSync(join(HERE, "expected-tools.json"), "utf8"),
);
const DRIVER = process.env.CUA_DRIVER_BINARY
  ? resolve(process.env.CUA_DRIVER_BINARY)
  : null;
const EXPECTED_VERSION = process.env.CUA_DRIVER_EXPECTED_VERSION;
const EXPECTED_TOOLS = sorted([
  ...EXPECTED.baseTools,
  ...(EXPECTED.platformTools[process.platform] ?? []),
]);
const DRIVER_ARGS = [
  "mcp",
  "--direct",
  "--dangerously-bypass-approvals",
];
const PROBE_ENV = {
  ...process.env,
  CUA_DRIVER_RS_TELEMETRY_ENABLED: "false",
  CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: "1",
  DISABLE_AUTOUPDATER: "1",
  NO_COLOR: "1",
};
const BIN_DIR = join(HERE, "node_modules", ".bin");
const CODEX = join(BIN_DIR, process.platform === "win32" ? "codex.cmd" : "codex");

function claudeBinary() {
  const platform = process.platform === "darwin" ? "darwin" : process.platform;
  const arch = process.arch === "x64" ? "x64" : process.arch;
  const executable = process.platform === "win32" ? "claude.exe" : "claude";
  return join(
    HERE,
    "node_modules",
    "@anthropic-ai",
    `claude-code-${platform}-${arch}`,
    executable,
  );
}

const CLAUDE = claudeBinary();

assert(DRIVER, "CUA_DRIVER_BINARY must name the extracted release candidate");
assert(EXPECTED_VERSION, "CUA_DRIVER_EXPECTED_VERSION must be set");

function sorted(values) {
  return [...values].sort();
}

function assertExactTools(label, tools) {
  const names = sorted(tools.map((tool) => tool.name));
  assert.deepEqual(
    names,
    EXPECTED_TOOLS,
    `${label} did not load the exact expected Cua Driver tool roster`,
  );
}

function assertWireSchemas(label, tools) {
  let outputSchemaCount = 0;
  for (const tool of tools) {
    assert.equal(
      tool.inputSchema?.type,
      "object",
      `${label}: ${tool.name} inputSchema must have an object root`,
    );
    if (tool.outputSchema !== undefined) {
      outputSchemaCount += 1;
      assert.equal(
        tool.outputSchema?.type,
        "object",
        `${label}: ${tool.name} outputSchema must have an object root`,
      );
    }
  }
  assert.equal(
    outputSchemaCount,
    EXPECTED.outputSchemaCount,
    `${label}: outputSchema coverage changed without updating the compatibility manifest`,
  );
}

function run(command, args, options = {}) {
  const result = spawnSync(command, args, {
    encoding: "utf8",
    env: PROBE_ENV,
    maxBuffer: 32 * 1024 * 1024,
    timeout: 60_000,
    ...options,
  });
  if (result.error) {
    throw result.error;
  }
  assert.equal(
    result.status,
    0,
    `${command} ${args.join(" ")} failed\nstdout:\n${result.stdout}\nstderr:\n${result.stderr}`,
  );
  return result;
}

async function rawPackagedProbe() {
  const child = spawn(DRIVER, DRIVER_ARGS, {
    env: PROBE_ENV,
    stdio: ["pipe", "pipe", "pipe"],
  });
  const stdout = createInterface({ input: child.stdout });
  let stderr = "";
  child.stderr.on("data", (chunk) => {
    stderr = `${stderr}${chunk}`.slice(-16_384);
  });

  const response = new Promise((resolveResponse, rejectResponse) => {
    const timeout = setTimeout(() => {
      rejectResponse(new Error(`raw MCP tools/list timed out\nstderr:\n${stderr}`));
    }, 30_000);
    stdout.on("line", (line) => {
      let message;
      try {
        message = JSON.parse(line);
      } catch {
        return;
      }
      if (message.id === 2) {
        clearTimeout(timeout);
        resolveResponse(message);
      }
    });
    child.once("error", rejectResponse);
    child.once("exit", (code) => {
      if (code !== null && code !== 0) {
        clearTimeout(timeout);
        rejectResponse(
          new Error(`raw MCP process exited ${code}\nstderr:\n${stderr}`),
        );
      }
    });
  });

  for (const message of [
    {
      jsonrpc: "2.0",
      id: 1,
      method: "initialize",
      params: {
        protocolVersion: "2025-11-25",
        capabilities: {},
        clientInfo: { name: "cua-release-probe", version: "1.0.0" },
      },
    },
    { jsonrpc: "2.0", method: "notifications/initialized" },
    { jsonrpc: "2.0", id: 2, method: "tools/list", params: {} },
  ]) {
    child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  try {
    const message = await response;
    assert(!message.error, `raw MCP tools/list failed: ${JSON.stringify(message.error)}`);
    const tools = message.result?.tools;
    assert(Array.isArray(tools), "raw MCP tools/list did not return result.tools");
    assertExactTools("raw packaged candidate", tools);
    assertWireSchemas("raw packaged candidate", tools);
    console.log(`raw packaged candidate: ${tools.length} tools loaded`);
  } finally {
    stdout.close();
    child.kill("SIGTERM");
  }
}

async function officialSdkProbe() {
  const transport = new StdioClientTransport({
    command: DRIVER,
    args: DRIVER_ARGS,
    env: PROBE_ENV,
    stderr: "pipe",
  });
  const client = new Client(
    { name: "cua-release-sdk-probe", version: "1.0.0" },
    { capabilities: {} },
  );
  try {
    await client.connect(transport);
    assert.equal(client.getServerVersion()?.name, EXPECTED.serverName);
    assert.equal(client.getServerVersion()?.version, EXPECTED_VERSION);
    const result = await client.listTools();
    assertExactTools("official TypeScript MCP SDK", result.tools);
    assertWireSchemas("official TypeScript MCP SDK", result.tools);
    console.log(`official TypeScript MCP SDK: ${result.tools.length} tools loaded`);
  } finally {
    await client.close();
  }
}

async function claudeCodeProbe() {
  const configDir = await mkdtemp(join(tmpdir(), "cua-claude-mcp-"));
  const debugFile = join(configDir, "mcp-debug.log");
  const env = { ...PROBE_ENV, CLAUDE_CONFIG_DIR: configDir };
  const version = run(CLAUDE, ["--version"], { env });
  assert.match(version.stdout, /2\.1\.224/);

  run(
    CLAUDE,
    [
      "mcp",
      "add",
      "--scope",
      "user",
      "cua-driver",
      "--",
      DRIVER,
      ...DRIVER_ARGS,
    ],
    { env },
  );
  const health = run(
    CLAUDE,
    ["--bare", "--debug-file", debugFile, "mcp", "get", "cua-driver"],
    { env },
  );
  assert.match(health.stdout, /Status:\s+.*Connected/);
  const debug = readFileSync(debugFile, "utf8");
  assert.match(debug, /"hasTools":true/);
  assert.match(
    debug,
    new RegExp(`"name":"${EXPECTED.serverName}","version":"${EXPECTED_VERSION.replaceAll(".", "\\.")}"`),
  );
  console.log("Claude Code 2.1.224: connected and accepted the complete tool schema set");
}

async function codexProbe() {
  const codexHome = await mkdtemp(join(tmpdir(), "cua-codex-mcp-"));
  const env = { ...PROBE_ENV, CODEX_HOME: codexHome };
  const version = run(CODEX, ["--version"], { env });
  assert.match(version.stdout, /0\.146\.1/);

  const commandConfig = `mcp_servers.cua_driver.command=${JSON.stringify(DRIVER)}`;
  const argsConfig = `mcp_servers.cua_driver.args=${JSON.stringify(DRIVER_ARGS)}`;
  const child = spawn(
    CODEX,
    [
      "app-server",
      "--listen",
      "stdio://",
      "-c",
      commandConfig,
      "-c",
      argsConfig,
    ],
    { env, stdio: ["pipe", "pipe", "pipe"] },
  );
  const stdout = createInterface({ input: child.stdout });
  let stderr = "";
  child.stderr.on("data", (chunk) => {
    stderr = `${stderr}${chunk}`.slice(-16_384);
  });
  const response = new Promise((resolveResponse, rejectResponse) => {
    const timeout = setTimeout(() => {
      rejectResponse(
        new Error(`Codex mcpServerStatus/list timed out\nstderr:\n${stderr}`),
      );
    }, 45_000);
    stdout.on("line", (line) => {
      let message;
      try {
        message = JSON.parse(line);
      } catch {
        return;
      }
      if (message.id === 2) {
        clearTimeout(timeout);
        resolveResponse(message);
      }
    });
    child.once("error", rejectResponse);
    child.once("exit", (code) => {
      if (code !== null && code !== 0) {
        clearTimeout(timeout);
        rejectResponse(
          new Error(`Codex app-server exited ${code}\nstderr:\n${stderr}`),
        );
      }
    });
  });

  for (const message of [
    {
      id: 1,
      method: "initialize",
      params: {
        clientInfo: { name: "cua-release-probe", version: "1.0.0" },
      },
    },
    { method: "initialized" },
    { id: 2, method: "mcpServerStatus/list", params: { detail: "full" } },
  ]) {
    child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  try {
    const message = await response;
    assert(!message.error, `Codex MCP discovery failed: ${JSON.stringify(message.error)}`);
    const server = message.result?.data?.find((item) => item.name === "cua_driver");
    assert(server, "Codex did not discover the configured cua_driver MCP server");
    assert.equal(server.serverInfo?.name, EXPECTED.serverName);
    assert.equal(server.serverInfo?.version, EXPECTED_VERSION);
    const tools = Object.values(server.tools ?? {});
    assertExactTools("Codex", tools);
    console.log(`Codex 0.146.1: ${tools.length} tools loaded`);
  } finally {
    stdout.close();
    child.kill("SIGTERM");
  }
}

const driverVersion = run(DRIVER, ["--version"]);
assert.match(
  driverVersion.stdout,
  new RegExp(`\\b${EXPECTED_VERSION.replaceAll(".", "\\.")}\\b`),
  "the extracted candidate version did not match the release version",
);
await rawPackagedProbe();
await officialSdkProbe();
await claudeCodeProbe();
await codexProbe();
console.log(
  `MCP release compatibility passed for Cua Driver ${EXPECTED_VERSION}`,
);
