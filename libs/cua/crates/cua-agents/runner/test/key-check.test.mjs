// A rejected API key ends the run's first turn in seconds, not after the
// harness's own retries (Claude Code retries a 401 ten times, about 3
// minutes, reporting nothing meanwhile). Runs the real runner against a fake
// harness that never answers its prompt (the stalled retries), a local HTTP
// server standing in for the provider's models list, and a stubbed ACP SDK.
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
const KEY = "sk-ant-test-0123456789abcdef";

// The ACP SDK, reduced to what the runner calls. The fake agent behind it
// answers initialize and session/new, and records every prompt in
// <run>/prompts.log. With STUB_PROMPT=hang a prompt never returns and keeps
// reporting "Retrying ... attempt n of 10" updates: the stalled harness.
function stubSdk(root) {
  const d = path.join(root, "node_modules", "@agentclientprotocol", "sdk");
  fs.mkdirSync(d, { recursive: true });
  fs.writeFileSync(path.join(d, "package.json"), JSON.stringify({ name: "@agentclientprotocol/sdk", type: "module", main: "index.js" }));
  fs.writeFileSync(
    path.join(d, "index.js"),
    `import fs from "node:fs";
     import path from "node:path";
     export const PROTOCOL_VERSION = 1;
     export const methods = {
       agent: { initialize: "initialize", authenticate: "authenticate",
         session: { new: "session/new", resume: "session/resume", load: "session/load", setMode: "session/set_mode", prompt: "session/prompt", cancel: "session/cancel" } },
       client: { session: { requestPermission: "session/request_permission", update: "session/update" } },
     };
     export const ndJsonStream = () => ({});
     export function client() {
       const handlers = {};
       const c = {
         onRequest: (m, h) => ((handlers[m] = h), c),
         onNotification: (m, h) => ((handlers[m] = h), c),
         async connectWith(_stream, fn) {
           const dir = process.argv[2];
           const ctx = {
             async request(m, p) {
               if (m === "initialize") return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] };
               if (m === "session/new") return { sessionId: "s-1" };
               if (m === "session/prompt") {
                 fs.appendFileSync(path.join(dir, "prompts.log"), JSON.stringify(p.prompt) + "\\n");
                 if (process.env.STUB_PROMPT === "hang") {
                   for (let n = 1; ; n++) {
                     handlers["session/update"]({ params: { update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text: "Retrying Claude, attempt " + n + " of 10" } } } });
                     await new Promise((r) => setTimeout(r, 500));
                   }
                 }
                 return { stopReason: "end_turn" };
               }
               return {};
             },
             async notify() {},
           };
           await fn(ctx);
         },
       };
       return c;
     }
     export default {};`,
  );
}

// A provider: answers every request with `reply(req)` and counts them.
async function provider(reply) {
  const seen = [];
  const server = http.createServer((req, res) => {
    seen.push({ url: req.url, key: req.headers["x-api-key"] ?? req.headers.authorization });
    reply(req, res);
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  return { seen, port: server.address().port, close: () => server.close() };
}

const json = (status, body) => (_req, res) => {
  res.writeHead(status, { "content-type": "application/json" });
  res.end(JSON.stringify(body));
};
const AUTH_401 = json(401, { type: "error", error: { type: "authentication_error", message: "API key is invalid." }, request_id: null });

// Runs the runner once; resolves with what it left behind.
async function run({ port, keyCheck = {}, env = {}, runEnv = {}, hang = true, key = KEY, timeoutMs = 20_000 }) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "cua-key-check-"));
  try {
    stubSdk(root);
    fs.copyFileSync(path.join(here, "..", "acp-runner.mjs"), path.join(root, "runner.mjs"));
    const dir = path.join(root, "run");
    fs.mkdirSync(path.join(dir, "inbox"), { recursive: true });
    fs.writeFileSync(
      path.join(dir, "run.json"),
      JSON.stringify({
        agent: { command: process.execPath, args: ["-e", "setInterval(() => {}, 1000)"] },
        cwd: dir,
        prompt: "Reply with the word OK.",
        exitWhenIdle: true,
        env,
        keyCheck: {
          provider: "Anthropic",
          url: `http://127.0.0.1:${port}/v1/models?limit=1`,
          keyEnv: "ANTHROPIC_API_KEY",
          headers: { "x-api-key": "${ANTHROPIC_API_KEY}", "anthropic-version": "2023-06-01" },
          skipIfEnv: ["ANTHROPIC_BASE_URL"],
          ...keyCheck,
        },
      }),
    );
    fs.writeFileSync(path.join(dir, "secrets.json"), JSON.stringify({ ANTHROPIC_API_KEY: key }));
    const t0 = Date.now();
    const child = spawn(process.execPath, [path.join(root, "runner.mjs"), dir], {
      stdio: "ignore",
      env: { ...process.env, ...(hang ? { STUB_PROMPT: "hang" } : {}), ...runEnv },
    });
    const code = await Promise.race([
      new Promise((r) => child.on("exit", r)),
      new Promise((r) => setTimeout(() => (child.kill("SIGKILL"), r("did not exit")), timeoutMs).unref()),
    ]);
    const read = (f) => {
      try {
        return fs.readFileSync(path.join(dir, f), "utf8");
      } catch {
        return "";
      }
    };
    return {
      code,
      ms: Date.now() - t0,
      events: read("events.jsonl").split("\n").filter(Boolean).map((l) => JSON.parse(l)),
      raw: read("events.jsonl") + read("state.json"),
      state: JSON.parse(read("state.json") || "{}"),
      prompts: read("prompts.log").split("\n").filter(Boolean).length,
    };
  } finally {
    fs.rmSync(root, { recursive: true, force: true });
  }
}

test("a rejected key ends the first turn in seconds with the provider's message, once", async () => {
  const p = await provider(AUTH_401);
  try {
    // The harness would stall on its retries (STUB_PROMPT=hang): the run
    // must end without ever sending it the prompt.
    const r = await run({ port: p.port });
    assert.equal(r.code, 0);
    assert.ok(r.ms < 10_000, `took ${r.ms} ms`);
    assert.equal(r.prompts, 0);
    const types = r.events.map((e) => e.type);
    assert.deepEqual(types.filter((t) => ["turn_started", "error", "turn_ended", "run_exited"].includes(t)), ["turn_started", "error", "turn_ended", "run_exited"]);
    const err = r.events.find((e) => e.type === "error");
    assert.equal(err.message, "Failed to authenticate. Anthropic API Error: 401 API key is invalid.");
    assert.equal(err.data.errorKind, "authentication_failed");
    assert.equal(r.events.find((e) => e.type === "turn_ended").stopReason, "error");
    assert.equal(r.state.stopReason, "error");
    // One request, carrying the key from the run's env.
    assert.deepEqual(p.seen, [{ url: "/v1/models?limit=1", key: KEY }]);
    // The key is in neither the events nor the state.
    assert.ok(!r.raw.includes(KEY));
  } finally {
    p.close();
  }
});

test("a provider message that echoes the key is redacted", async () => {
  const key = "plain-key-0123456789abcdef";
  const p = await provider(json(401, { type: "error", error: { type: "authentication_error", message: `invalid x-api-key ${key}` } }));
  try {
    const r = await run({ port: p.port, key });
    const err = r.events.find((e) => e.type === "error");
    assert.equal(err.message, "Failed to authenticate. Anthropic API Error: 401 invalid x-api-key [redacted]");
    assert.ok(!r.raw.includes(key));
  } finally {
    p.close();
  }
});

test("OpenAI's masked copy of the key is dropped from the message", async () => {
  const masked = "Incorrect API key provided: sk-test-*********************************a401. You can find your API key at https://platform.openai.com/account/api-keys.";
  const p = await provider(json(401, { error: { message: masked, type: "invalid_request_error", param: null, code: "invalid_api_key" } }));
  try {
    const r = await run({ port: p.port, keyCheck: { provider: "OpenAI" } });
    const err = r.events.find((e) => e.type === "error");
    assert.equal(err.message, "Failed to authenticate. OpenAI API Error: 401 Incorrect API key provided.");
  } finally {
    p.close();
  }
});

test("a 403 that says the key is invalid also ends the turn", async () => {
  const p = await provider(json(403, { type: "error", error: { type: "authentication_error", message: "API key is invalid." } }));
  try {
    const r = await run({ port: p.port });
    assert.equal(r.prompts, 0);
    assert.match(r.events.find((e) => e.type === "error").message, /API Error: 403 API key is invalid/);
  } finally {
    p.close();
  }
});

test("an accepted key sends the prompt as before, with no error", async () => {
  const p = await provider(json(200, { data: [], has_more: false }));
  try {
    const r = await run({ port: p.port, hang: false });
    assert.equal(r.code, 0);
    assert.equal(r.prompts, 1);
    assert.ok(!r.events.some((e) => e.type === "error"));
    assert.equal(r.state.stopReason, "end_turn");
  } finally {
    p.close();
  }
});

test("anything but a definitive rejection lets the run go on", async () => {
  const cases = {
    "a 500": json(500, { type: "error", error: { type: "api_error", message: "boom" } }),
    "a 429": json(429, { type: "error", error: { type: "rate_limit_error", message: "slow down" } }),
    "a 403 for a missing permission": json(403, { error: { type: "permission_error", message: "no access to models" } }),
    "a 401 without the provider's error body": (_req, res) => (res.writeHead(401, { "content-type": "text/html" }), res.end("<h1>proxy login</h1>")),
    "no answer within the timeout": () => {},
  };
  for (const [name, reply] of Object.entries(cases)) {
    const p = await provider(reply);
    try {
      const r = await run({ port: p.port, hang: false, keyCheck: { timeoutMs: 400 } });
      assert.equal(r.prompts, 1, name);
      assert.ok(!r.events.some((e) => e.type === "error"), name);
    } finally {
      p.close();
    }
  }
  // Nothing listening.
  const p = await provider(AUTH_401);
  const port = p.port;
  p.close();
  const r = await run({ port, hang: false });
  assert.equal(r.prompts, 1, "connection refused");
});

test("no key, or a backend override, skips the check", async () => {
  const p = await provider(AUTH_401);
  try {
    // A base URL: the key goes to that endpoint, not the provider.
    let r = await run({ port: p.port, hang: false, env: { ANTHROPIC_BASE_URL: "http://proxy.invalid" } });
    assert.equal(r.prompts, 1);
    // A key the run does not have.
    r = await run({ port: p.port, hang: false, keyCheck: { keyEnv: "NOT_SET_ANYWHERE" } });
    assert.equal(r.prompts, 1);
    // A sign-in file that may stand in for the key.
    const signedIn = path.join(os.tmpdir(), `cua-key-check-login-${process.pid}.json`);
    fs.writeFileSync(signedIn, "{}");
    try {
      r = await run({ port: p.port, hang: false, keyCheck: { skipIfFile: [signedIn] } });
      assert.equal(r.prompts, 1);
    } finally {
      fs.rmSync(signedIn, { force: true });
    }
    // None of the three asked the provider.
    assert.deepEqual(p.seen, []);
  } finally {
    p.close();
  }
});
