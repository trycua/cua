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
