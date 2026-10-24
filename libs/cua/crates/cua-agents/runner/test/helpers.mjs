// Shared by the durable runner tests: finds @agentclientprotocol/sdk, builds a
// sandbox directory around the real runner and runs it as a process.
import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const here = path.dirname(fileURLToPath(import.meta.url));
export const RUNNER = path.join(here, "..", "acp-runner.mjs");
const SDK_VERSION = "1.5.0";

// A directory whose node_modules has the pinned SDK: $ACP_SDK_DIR, the cua
// tools install, or a one-time npm install in the temp dir. null: no SDK.
export function sdkRoot() {
  const candidates = [
    process.env.ACP_SDK_DIR,
    path.join(os.homedir(), ".cua", "tools", "acp-sdk", SDK_VERSION),
    path.join(os.tmpdir(), `cua-acp-sdk-${SDK_VERSION}`),
  ].filter(Boolean);
  const has = (d) => fs.existsSync(path.join(d, "node_modules", "@agentclientprotocol", "sdk", "package.json"));
  for (const c of candidates) if (has(c)) return c;
  const target = candidates[candidates.length - 1];
  fs.mkdirSync(target, { recursive: true });
  const r = spawnSync("npm", ["install", "--no-audit", "--no-fund", "--prefix", target, `@agentclientprotocol/sdk@${SDK_VERSION}`], { stdio: "ignore" });
  return r.status === 0 && has(target) ? target : null;
}

export class Box {
  constructor(sdk) {
    this.root = fs.mkdtempSync(path.join(process.env.CUA_TEST_TMP ?? os.tmpdir(), "cua-durable-"));
    fs.symlinkSync(path.join(sdk, "node_modules"), path.join(this.root, "node_modules"));
    fs.copyFileSync(RUNNER, path.join(this.root, "runner.mjs"));
    fs.copyFileSync(path.join(here, "mock-acp-agent.mjs"), path.join(this.root, "mock-acp-agent.mjs"));
    this.run = path.join(this.root, "run");
    this.state = path.join(this.root, "state");
    this.mock = path.join(this.root, "mock");
    for (const d of [this.run, this.mock]) fs.mkdirSync(d, { recursive: true });
    this.procs = new Set();
  }
  script(s) {
    fs.writeFileSync(path.join(this.mock, "script.json"), JSON.stringify(s));
  }
  config(extra = {}) {
    fs.writeFileSync(
      path.join(this.run, "run.json"),
      JSON.stringify({
        agent: { command: process.execPath, args: [path.join(this.root, "mock-acp-agent.mjs")] },
        cwd: this.run,
        prompt: "do the thing",
        env: { MOCK_DIR: this.mock, MOCK_SCRIPT: path.join(this.mock, "script.json"), MOCK_STATE_DIR: this.state },
        durable: { stateDir: this.state, ...(extra.durable ?? {}) },
        ...Object.fromEntries(Object.entries(extra).filter(([k]) => k !== "durable")),
      }),
    );
  }
  // Runs the runner once; resolves {code, signal, stderr}.
  go({ epoch, resume = true, env = {}, timeoutMs = 25_000 } = {}) {
    const p = spawn(process.execPath, [path.join(this.root, "runner.mjs"), this.run], {
      env: { ...process.env, CUA_SESSION: "s1", CUA_SESSION_EPOCH: String(epoch), CUA_SESSION_RESUME: resume ? "1" : "0", CUA_SESSION_STATE_DIR: this.state, ...env },
      stdio: ["ignore", "pipe", "pipe"],
    });
    this.procs.add(p);
    let stderr = "";
    let stdout = "";
    p.stderr.on("data", (d) => (stderr += d));
    p.stdout.on("data", (d) => (stdout += d));
    return new Promise((resolve) => {
      const t = setTimeout(() => {
        p.kill("SIGKILL");
        resolve({ code: "timeout", signal: null, stderr, stdout });
      }, timeoutMs);
      p.on("exit", (code, signal) => {
        clearTimeout(t);
        this.procs.delete(p);
        resolve({ code, signal, stderr, stdout });
      });
    });
  }
  lines(file, dirName = this.state) {
    try {
      return fs
        .readFileSync(path.join(dirName, file), "utf8")
        .split("\n")
        .filter(Boolean)
        .map((l) => JSON.parse(l));
    } catch {
      return [];
    }
  }
  events() {
    return this.lines("events.jsonl");
  }
  tools() {
    return this.lines("tools.jsonl");
  }
  agentLog() {
    return this.lines("log.jsonl", this.mock);
  }
  prompts() {
    return this.agentLog().filter((l) => l.ev === "prompt").map((l) => l.text);
  }
  effects() {
    try {
      return fs.readFileSync(path.join(this.mock, "effects.log"), "utf8").split("\n").filter(Boolean);
    } catch {
      return [];
    }
  }
  cleanup() {
    for (const p of this.procs) p.kill("SIGKILL");
    fs.rmSync(this.root, { recursive: true, force: true });
  }
}
