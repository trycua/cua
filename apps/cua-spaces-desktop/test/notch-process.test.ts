// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { spawn as realSpawn, type ChildProcess } from "node:child_process";
import { EventEmitter } from "node:events";
import { PassThrough } from "node:stream";
import { describe, expect, it, vi } from "vitest";
import { helperEnv, NotchProcess, type Spawn } from "../src/notch/process";
import { MISMATCH_EXIT, type HelperMessage } from "../src/notch/protocol";
import { manualClock } from "./fakeNotchCore";

/** A helper process that never runs anything: its pipes are in memory. */
class FakeChild extends EventEmitter {
  stdin = new PassThrough();
  stdout = new PassThrough();
  stderr = new PassThrough();
  exitCode: number | null = null;
  signalCode: NodeJS.Signals | null = null;
  killed: string[] = [];
  pid = 4242;
  written = "";

  constructor() {
    super();
    this.stdin.setEncoding("utf8");
    this.stdin.on("data", (c: string) => (this.written += c));
  }

  /** What the helper read, one message per line. */
  get received(): unknown[] {
    return this.written.split("\n").filter(Boolean).map((l) => JSON.parse(l));
  }

  say(message: unknown): void {
    this.stdout.write(`${JSON.stringify(message)}\n`);
  }

  exit(code: number | null, signal: NodeJS.Signals | null = null): void {
    this.exitCode = code;
    this.signalCode = signal;
    this.emit("exit", code, signal);
  }

  kill(signal: string): boolean {
    this.killed.push(signal);
    return true;
  }
}

function setup(backoff = { initialMs: 500, maxMs: 4000, stableMs: 60_000, maxFailures: 4 }) {
  const clock = manualClock();
  const children: FakeChild[] = [];
  const spawned: { command: string; options: unknown }[] = [];
  const spawn: Spawn = (command, _args, options) => {
    spawned.push({ command, options });
    const c = new FakeChild();
    children.push(c);
    return c as unknown as ChildProcess;
  };
  const messages: HelperMessage[] = [];
  const logs: string[] = [];
  let starts = 0;
  const p = new NotchProcess({
    path: "/Applications/Cua Spaces.app/Contents/Helpers/Cua Spaces Notch.app/Contents/MacOS/Cua Spaces Notch",
    spawn,
    backoff,
    after: clock.after,
    now: clock.now,
    log: (m) => logs.push(m),
    onMessage: (m) => messages.push(m),
    onStart: () => {
      starts++;
      p.send({ type: "ghost" });
    },
  });
  return { p, clock, children, spawned, messages, logs, starts: () => starts };
}

describe("notch helper process", () => {
  it("spawns the helper with pipes and a minimal environment, and greets it", async () => {
    const { p, children, spawned, starts } = setup();
    p.start();
    expect(p.running).toBe(true);
    expect(spawned[0]?.options).toMatchObject({ stdio: ["pipe", "pipe", "pipe"] });
    const env = (spawned[0]?.options as { env: NodeJS.ProcessEnv }).env;
    expect(Object.keys(env).every((k) => ["HOME", "PATH", "TMPDIR", "USER", "LOGNAME", "LANG", "LC_ALL"].includes(k))).toBe(true);
    expect(starts()).toBe(1);
    await vi.waitFor(() => expect(children[0]!.received).toEqual([{ type: "ghost" }]));
  });

  it("keeps only the environment AppKit needs", () => {
    expect(helperEnv({ HOME: "/u", PATH: "/bin", CUA_TOKEN: "secret", ELECTRON_RUN_AS_NODE: "1" })).toEqual({ HOME: "/u", PATH: "/bin" });
  });

  it("reads the helper's lines, skipping malformed ones, and logs its stderr", async () => {
    const { p, children, messages, logs } = setup();
    p.start();
    const c = children[0]!;
    c.say({ type: "hello", v: 1, pid: 4242 });
    c.stdout.write('{"type":"stage","open":tr');
    c.stdout.write('ue}\nnot json\n{"type":"event","event":{"kind":"click"}}\n');
    c.stderr.write("cua-spaces-notch: bad message\n");
    await vi.waitFor(() => expect(messages).toHaveLength(3));
    expect(messages).toEqual([
      { type: "hello", v: 1, pid: 4242 },
      { type: "stage", open: true },
      { type: "event", event: { kind: "click" } },
    ]);
    expect(logs).toContain("cua-spaces-notch: bad message");
    expect(logs.some((l) => l.includes("malformed"))).toBe(true);
  });

  it("restarts a crashed helper with a growing delay, then gives up", () => {
    const { p, clock, children, logs } = setup();
    p.start();
    children[0]!.exit(null, "SIGSEGV");
    expect(p.running).toBe(false);
    clock.advance(499);
    expect(children).toHaveLength(1);
    clock.advance(1);
    expect(children).toHaveLength(2);
    children[1]!.exit(1);
    clock.advance(1000);
    expect(children).toHaveLength(3);
    children[2]!.exit(1);
    clock.advance(2000);
    expect(children).toHaveLength(4);
    children[3]!.exit(1);
    clock.advance(4000);
    expect(children).toHaveLength(5);
    // The fifth short run in a row: no more restarts.
    children[4]!.exit(1);
    clock.advance(60_000);
    expect(children).toHaveLength(5);
    expect(logs.at(-1)).toMatch(/giving up/);
  });

  it("starts the delay over after a long run", () => {
    const { p, clock, children } = setup();
    p.start();
    children[0]!.exit(1);
    clock.advance(500);
    clock.advance(120_000);
    children[1]!.exit(1);
    clock.advance(500);
    expect(children).toHaveLength(3);
  });

  it("never restarts a helper that speaks another protocol", () => {
    const { p, clock, children, logs } = setup();
    p.start();
    children[0]!.exit(MISMATCH_EXIT);
    clock.advance(60_000);
    expect(children).toHaveLength(1);
    expect(logs.at(-1)).toMatch(/another notch protocol/);
  });

  it("restarts after a failed spawn too", () => {
    const { p, clock, children } = setup();
    p.start();
    children[0]!.emit("error", new Error("spawn ENOENT"));
    expect(p.running).toBe(false);
    clock.advance(500);
    expect(children).toHaveLength(2);
  });

  it("stops: asks the helper to quit, closes its stdin, kills it only if it lingers, and never restarts", async () => {
    const { p, clock, children } = setup();
    p.start();
    const c = children[0]!;
    const ended = new Promise((r) => c.stdin.on("finish", r));
    p.stop();
    await ended;
    expect(c.received.at(-1)).toEqual({ type: "quit" });
    clock.advance(2000);
    expect(c.killed).toEqual(["SIGKILL"]);
    c.exit(null, "SIGKILL");
    clock.advance(60_000);
    expect(children).toHaveLength(1);
    // Sending to a stopped helper is a no-op.
    p.send({ type: "quit" });
  });

  it("does not kill a helper that quit in time", () => {
    const { p, clock, children } = setup();
    p.start();
    p.stop();
    children[0]!.exit(0);
    clock.advance(5000);
    expect(children[0]!.killed).toEqual([]);
  });

  it("exits with the pipe: a real child that ends at stdin's EOF is gone when the app stops it", async () => {
    // A stand-in for the helper's contract (it exits when stdin closes),
    // with real pipes: greet, echo the hello, exit on EOF.
    const script = [
      "process.stdout.write(JSON.stringify({ type: 'hello', v: 1, pid: process.pid }) + '\\n');",
      "process.stdin.on('data', () => {});",
      "process.stdin.on('end', () => process.exit(0));",
    ].join("");
    const messages: HelperMessage[] = [];
    let exited = false;
    const p = new NotchProcess({
      path: process.execPath,
      spawn: (command, _args, options) => {
        const child = realSpawn(command, ["-e", script], options);
        child.on("exit", () => (exited = true));
        return child;
      },
      env: { PATH: process.env.PATH },
      onMessage: (m) => messages.push(m),
      onStart: () => {},
      log: () => {},
    });
    p.start();
    await vi.waitFor(() => expect(messages[0]).toMatchObject({ type: "hello", v: 1 }), { timeout: 5000 });
    p.stop();
    await vi.waitFor(() => expect(exited).toBe(true), { timeout: 5000 });
    expect(p.running).toBe(false);
  });
});
