// A run that cannot start says why. Node words a missing working folder, a
// missing program and a missing shebang interpreter all as
// `spawn <program> ENOENT`; the runner names which one it is.
//
//   node --test libs/cua/crates/cua-agents/runner/test/
import { test } from "node:test";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));

async function startFailure(t, { cwd, command, args = [], env = {}, setup }) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "cua-enoent-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const sdk = path.join(root, "node_modules", "@agentclientprotocol", "sdk");
  fs.mkdirSync(sdk, { recursive: true });
  fs.writeFileSync(path.join(sdk, "package.json"), JSON.stringify({ name: "@agentclientprotocol/sdk", type: "module", main: "index.js" }));
  fs.writeFileSync(
    path.join(sdk, "index.js"),
    `const any = new Proxy(function () {}, { get: (_, k) => (k === "then" ? undefined : any), apply: () => any });
     export const ndJsonStream = any, methods = any, PROTOCOL_VERSION = 1, client = any;
     export default any;`,
  );
  fs.copyFileSync(path.join(here, "..", "acp-runner.mjs"), path.join(root, "runner.mjs"));
  const run = path.join(root, "run");
  fs.mkdirSync(run);
  setup?.(root);
  fs.writeFileSync(
    path.join(run, "run.json"),
    JSON.stringify({ cwd: cwd(root), agent: { command: command(root), args }, env: env && Object.fromEntries(Object.entries(env).map(([k, v]) => [k, typeof v === "function" ? v(root) : v])) }),
  );
  const child = spawn(process.execPath, [path.join(root, "runner.mjs"), run], { stdio: "ignore" });
  t.after(() => child.kill("SIGKILL"));
  await new Promise((resolve) => child.on("exit", resolve));
  return JSON.parse(fs.readFileSync(path.join(run, "state.json"), "utf8"));
}

test("a working folder that cannot be made is named, not the program", async (t) => {
  const st = await startFailure(t, {
    cwd: (root) => path.join(root, "gone", "work"),
    command: () => process.execPath,
    args: ["-e", "0"],
    // A file where the parent folder should be: mkdir -p fails.
    setup: (root) => fs.writeFileSync(path.join(root, "gone"), "x"),
  });
  assert.equal(st.status, "failed");
  assert.match(st.error, /^The working folder .*work is not available in this Space/);
});

test("a missing working folder is made again before the agent starts", async (t) => {
  const st = await startFailure(t, {
    cwd: (root) => path.join(root, "again", "work"),
    command: () => process.execPath,
    args: ["-e", "0"],
  });
  assert.doesNotMatch(st.error ?? "", /working folder/);
});

test("a script whose node is not installed says Node is not installed", async (t) => {
  const st = await startFailure(t, {
    cwd: (root) => path.join(root, "run"),
    command: (root) => path.join(root, "agent.js"),
    env: { PATH: (root) => path.join(root, "empty") },
    setup: (root) => {
      fs.mkdirSync(path.join(root, "empty"));
      fs.writeFileSync(path.join(root, "agent.js"), "#!/usr/bin/env node\nconsole.log(1)\n", { mode: 0o755 });
    },
  });
  assert.match(st.error, /^Node is not installed in this Space\./);
});

test("a program that is not there says it is not installed", async (t) => {
  const st = await startFailure(t, {
    cwd: (root) => path.join(root, "run"),
    command: (root) => path.join(root, "nope"),
  });
  assert.match(st.error, /nope is not installed in this Space\./);
});
