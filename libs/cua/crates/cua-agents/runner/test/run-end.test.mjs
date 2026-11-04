// The runner ends its guest MCP agent session when the run ends, so the
// run's presence cursor leaves at once. Runs the real runner against a fake
// agent that exits immediately and a local HTTP server standing in for
// cua-spacesd /mcp. The ACP SDK is stubbed (the agent never speaks ACP).
//
//   node --test libs/cua/crates/cua-agents/runner/test/
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));

function stubSdk(root) {
  const d = path.join(root, "node_modules", "@agentclientprotocol", "sdk");
  fs.mkdirSync(d, { recursive: true });
  fs.writeFileSync(path.join(d, "package.json"), JSON.stringify({ name: "@agentclientprotocol/sdk", type: "module", main: "index.js" }));
  // Every property and call returns the same inert chainable object.
  fs.writeFileSync(
    path.join(d, "index.js"),
    `const any = new Proxy(function () {}, { get: (_, k) => (k === "then" ? undefined : any), apply: () => any });
     export const ndJsonStream = any, methods = any, PROTOCOL_VERSION = 1, client = any;
     export default any;`,
  );
}

async function failureRun(t, { request = "return new Promise(() => {});", config = {}, agent = "setInterval(() => {}, 1000)", previous = {}, log = "", sdk = null } = {}) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "cua-failure-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  stubSdk(root);
  const sdkDir = path.join(root, "node_modules/@agentclientprotocol/sdk");
  if (sdk) {
    fs.rmSync(sdkDir, { recursive: true });
    fs.symlinkSync(sdk, sdkDir);
  } else {
    fs.writeFileSync(path.join(sdkDir, "index.js"), `
      export const PROTOCOL_VERSION = 1;
      export const methods = {agent: {initialize: 'initialize', authenticate: 'authenticate', session: {new: 'new', load: 'load', resume: 'resume', prompt: 'prompt'}}, client: {session: {requestPermission: 'permission', update: 'update'}}};
      export const ndJsonStream = () => null;
      export function client() { return {onRequest() {return this}, onNotification() {return this}, connectWith: async (_, callback) => callback({request: async (method, params) => {${request}}})}; }
    `);
  }
  fs.copyFileSync(path.join(here, "..", "acp-runner.mjs"), path.join(root, "runner.mjs"));
  const run = path.join(root, "run");
  fs.mkdirSync(run);
  fs.writeFileSync(path.join(run, "run.json"), JSON.stringify({ cwd: run, agent: {command: process.execPath, args: ["-e", agent]}, startupTimeoutMs: 1000, ...config }));
  fs.writeFileSync(path.join(run, "state.json"), JSON.stringify(previous));
  fs.writeFileSync(path.join(run, "agent.log"), log);
  fs.writeFileSync(path.join(run, "secrets.json"), JSON.stringify({ TOKEN: "secret-12345678" }));
  const child = spawn(process.execPath, [path.join(root, "runner.mjs"), run], {stdio: ["ignore", "ignore", "pipe"], detached: true});
  const kill = () => { try { process.kill(-child.pid, "SIGKILL"); } catch {} };
  t.after(kill);
  let stderr = "";
  child.stderr.on("data", (data) => stderr += data);
  await new Promise((resolve, reject) => {
    const timer = setTimeout(() => { kill(); reject(new Error(`runner hung: ${stderr}`)); }, 4000);
    child.on("exit", () => {clearTimeout(timer); resolve();});
  });
  return {events: fs.readFileSync(path.join(run, "events.jsonl"), "utf8").trim().split("\n").map(JSON.parse), state: JSON.parse(fs.readFileSync(path.join(run, "state.json"), "utf8"))};
}

for (const [name, options, expected] of [
  ["missing executable", {config: {agent: {command: "/missing/cua-agent"}}}, /ENOENT.*spawn: \/missing\/cua-agent[\s\S]*PATH/],
  ["missing interpreter with restricted PATH", {config: {agent: {command: "/usr/bin/env", args: ["cua-missing-interpreter"]}, env: {PATH: "/empty"}}}, /cua-missing-interpreter/],
  ["adapter installation failure", {agent: "console.error('agent not installed'); process.exit(1)", log: "old launch\n"}, /agent not installed/],
  ["initialize timeout", {config: {startupTimeoutMs: 100}}, /timed out.*initialize/],
  ["session timeout", {request: "if (method === 'initialize') return {}; return new Promise(() => {});", config: {startupTimeoutMs: 100}}, /timed out.*session/],
  ["authentication timeout", {request: "if (method === 'initialize') return {authMethods: [{id: 'login'}]}; return new Promise(() => {});", config: {authMethod: "login", startupTimeoutMs: 100}}, /timed out.*authenticate/],
  ["ACP structured error", {request: "while (!(await import('node:fs')).readFileSync(process.argv[2] + '/agent.log', 'utf8').includes('current launch')) await new Promise(r => setTimeout(r, 10)); throw Object.assign(new Error('internal error secret-12345678'), {code: -32603, data: {cause: 'adapter unavailable'}});", agent: "console.error('discarded prefix' + 'x'.repeat(10000) + 'current launch'); setInterval(() => {}, 1000)", config: {startupTimeoutMs: 1000}, log: "old launch\n"}, /-32603[\s\S]*adapter unavailable[\s\S]*current launch/],
  ["authentication required", {request: "if (method === 'initialize') return {agentCapabilities: {mcpCapabilities: {http: true}}}; throw Object.assign(new Error('Authentication required'), {code: -32000});", config: {authHint: "Run ~/.cua/bin/claude inside this bot's Space", mcpServers: [{type: "http", name: "test", url: "http://localhost"}]}}, /Authentication required[\s\S]*~\/\.cua\/bin\/claude.*Space/],
]) {
  test(name + " is diagnostic and terminal", {timeout: 6000}, async (t) => {
    const {events, state} = await failureRun(t, options);
    const errors = events.filter(e => e.type === "error");
    assert.equal(errors.length, 1);
    assert.match(errors[0].message, expected);
    assert.equal(events.filter(e => e.type === "run_exited").length, 1);
    assert.equal(state.status, "failed");
    assert.equal(JSON.stringify({events, state}).includes("secret-12345678"), false);
    assert.equal(errors[0].message.includes("old launch"), false);
    assert.equal(errors[0].message.includes("discarded prefix"), false);
    assert.ok(errors[0].message.length < 6000);
    assert.equal(errors[0].message.includes("PATH="), name === "missing executable");
    if (name === "authentication required") assert.equal(events.some(e => e.type === "notice"), false);
  });
}

test("prompt errors retain the provider reason without launch diagnostics", {timeout: 6000}, async (t) => {
  const {events} = await failureRun(t, {
    config: {prompt: "hello", idleExitSecs: 0.01},
    request: `if (method === 'initialize') return {};
      if (method === 'new') return {sessionId: 's'};
      while (!(await import('node:fs')).readFileSync(process.argv[2] + '/agent.log', 'utf8').includes('earlier turn stderr')) await new Promise(r => setTimeout(r, 10));
      throw Object.assign(new Error('Provider quota exceeded'), {code: -32001, data: {retryAfter: 60}});`,
    agent: "console.error('earlier turn stderr'); setInterval(() => {}, 1000)",
  });
  const errors = events.filter(e => e.type === "error");
  assert.equal(errors.length, 1);
  assert.equal(errors[0].message, "Provider quota exceeded");
  assert.equal(errors[0].code, -32001);
  assert.deepEqual(errors[0].data, {retryAfter: 60});
  assert.equal(errors[0].message.includes("PATH="), false);
  assert.equal(errors[0].message.includes("earlier turn stderr"), false);
  assert.equal(events.filter(e => e.type === "turn_ended").length, 1);
});

test("prompt authentication errors include the guest sign-in command and end the run as failed, so the next message starts a fresh one", {timeout: 6000}, async (t) => {
  const authHint = "Run ~/.cua/bin/claude inside the Space to sign in";
  const {events, state} = await failureRun(t, {
    config: {prompt: "hello", idleExitSecs: 0.01, authHint},
    request: `if (method === 'initialize') return {};
      if (method === 'new') return {sessionId: 's'};
      throw Object.assign(new Error('Authentication required'), {code: -32000});`,
  });
  const errors = events.filter(e => e.type === "error");
  assert.equal(errors.length, 1);
  assert.equal(errors[0].message, `Authentication required\n${authHint}`);
  assert.equal(errors[0].code, -32000);
  assert.equal(events.filter(e => e.type === "turn_ended").length, 1);
  // Not idle for half an hour with a credential that does not work: failed,
  // with the error kept, so the owner's next message starts a new run.
  assert.equal(events.filter(e => e.type === "run_exited").length, 1);
  assert.equal(state.status, "failed");
  assert.match(state.error, /Authentication required/);
});

test("other prompt errors still leave the run waiting for the next message", {timeout: 6000}, async (t) => {
  const {state} = await failureRun(t, {
    config: {prompt: "hello", idleExitSecs: 0.01},
    request: `if (method === 'initialize') return {};
      if (method === 'new') return {sessionId: 's'};
      throw Object.assign(new Error('Provider quota exceeded'), {code: -32001});`,
  });
  assert.equal(state.status, "exited");
});

test("transport exit ends a pending prompt exactly once", {timeout: 6000}, async (t) => {
  const {events, state} = await failureRun(t, {config: {prompt: "hello"}, request: "if (method === 'initialize') return {}; if (method === 'new') return {sessionId: 's'}; return new Promise(() => {});", agent: "setTimeout(() => process.exit(1), 300)"});
  assert.equal(events.filter(e => e.type === "turn_ended").length, 1);
  assert.equal(state.status, "failed");
});

for (const [name, agent, prompt] of [
  ["stdout closure", "setTimeout(() => process.stdout.end(), 300); setInterval(() => {}, 1000)", "return new Promise(() => {});"],
  ["late prompt rejection", "setTimeout(() => process.exit(1), 300)", "await new Promise(r => setTimeout(r, 400)); throw new Error('transport lost');"],
  ["late prompt success", "setTimeout(() => process.exit(1), 300)", "await new Promise(r => setTimeout(r, 400)); return {stopReason: 'end_turn'};"],
]) {
  test(name + " cannot duplicate or overwrite the terminal turn", {timeout: 6000}, async (t) => {
    const {events, state} = await failureRun(t, {agent, config: {prompt: "hello"}, request: `if (method === 'initialize') return {}; if (method === 'new') return {sessionId: 's'}; ${prompt}`});
    assert.equal(events.filter(e => e.type === "error").length, 1);
    assert.equal(events.filter(e => e.type === "turn_ended").length, 1);
    assert.equal(state.status, "failed");
    assert.equal(state.stopReason, "error");
  });
}

test("real SDK disconnect ends a pending prompt", {timeout: 6000}, async (t) => {
  if (!process.env.CUA_ACP_SDK_DIR) return t.skip("Set CUA_ACP_SDK_DIR to the installed @agentclientprotocol/sdk directory");
  const agent = `require('node:readline').createInterface({input: process.stdin}).on('line', line => {
    const m = JSON.parse(line);
    if (m.method === 'session/prompt') return setTimeout(() => process.exit(1), 30);
    const result = m.method === 'initialize' ? {protocolVersion: 1, agentCapabilities: {}} : {sessionId: 's'};
    process.stdout.write(JSON.stringify({jsonrpc: '2.0', id: m.id, result}) + '\\n');
  });`;
  const {events, state} = await failureRun(t, {sdk: process.env.CUA_ACP_SDK_DIR, agent, config: {prompt: "hello", startupTimeoutMs: 1000}});
  assert.equal(events.filter(e => e.type === "turn_ended").length, 1);
  assert.equal(state.status, "failed");
});

test("run end sends DELETE /mcp with the run's agent session", async () => {
  const seen = [];
  const server = http.createServer((req, res) => {
    seen.push({ method: req.method, session: req.headers["x-cua-agent-session"], auth: req.headers.authorization });
    res.writeHead(204).end();
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const port = server.address().port;
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "cua-runner-"));
  try {
    stubSdk(root);
    fs.copyFileSync(path.join(here, "..", "acp-runner.mjs"), path.join(root, "runner.mjs"));
    const run = path.join(root, "run");
    fs.mkdirSync(path.join(run, "inbox"), { recursive: true });
    fs.writeFileSync(
      path.join(run, "run.json"),
      JSON.stringify({
        agent: { command: process.execPath, args: ["-e", "process.exit(0)"] },
        cwd: run,
        mcpServers: [
          {
            type: "http",
            name: "cua-driver",
            url: `http://127.0.0.1:${port}/mcp`,
            headers: [
              { name: "Authorization", value: "${CUA_MCP_0_0}" },
              { name: "X-Cua-Agent-Session", value: "${CUA_MCP_0_1}" },
            ],
          },
          { type: "http", name: "other", url: `http://127.0.0.1:${port}/other`, headers: [] },
        ],
      }),
    );
    fs.writeFileSync(path.join(run, "secrets.json"), JSON.stringify({ CUA_MCP_0_0: "Bearer tok-0123456789", CUA_MCP_0_1: "run-84a8dc1f" }));
    const child = spawn(process.execPath, [path.join(root, "runner.mjs"), run], { stdio: "ignore" });
    const code = await Promise.race([
      new Promise((r) => child.on("exit", r)),
      new Promise((_, j) => setTimeout(() => j(new Error("runner did not exit within 10 s")), 10_000).unref()),
    ]);
    assert.notEqual(code, null);
    assert.deepEqual(seen, [{ method: "DELETE", session: "run-84a8dc1f", auth: "Bearer tok-0123456789" }]);
  } finally {
    server.close();
    fs.rmSync(root, { recursive: true, force: true });
  }
});
