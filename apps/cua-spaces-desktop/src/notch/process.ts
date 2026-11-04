// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The notch helper process: spawned with pipes on stdin, stdout and stderr,
// spoken to in the notch protocol, restarted with a growing delay when it
// crashes. It exits by itself when its stdin closes, so quitting or crashing
// this app never leaves it behind. Pure Node (no Electron), so it is tested
// with a fake helper.

import { spawn as nodeSpawn, type ChildProcess, type SpawnOptions } from "node:child_process";
import { encodeMessage, LineSplitter, MISMATCH_EXIT, parseHelperMessage, type HelperMessage, type HostMessage } from "./protocol";

export type Spawn = (command: string, args: readonly string[], options: SpawnOptions) => ChildProcess;

export interface Backoff {
  /** The first restart's delay. */
  initialMs: number;
  /** The longest delay. */
  maxMs: number;
  /** A run this long resets the delay. */
  stableMs: number;
  /** Consecutive short runs before giving up. */
  maxFailures: number;
}

export const DEFAULT_BACKOFF: Backoff = { initialMs: 500, maxMs: 30_000, stableMs: 60_000, maxFailures: 10 };

export interface NotchProcessOptions {
  /** The helper's executable. */
  path: string;
  /** Its environment (default: a minimal one). */
  env?: NodeJS.ProcessEnv;
  spawn?: Spawn;
  backoff?: Backoff;
  /** Timers (tests step them). */
  after?(ms: number, fire: () => void): () => void;
  now?(): number;
  /** A message from the helper. */
  onMessage(message: HelperMessage): void;
  /** A helper (re)started: send it hello and the state. */
  onStart(): void;
  log?(message: string): void;
}

/** The environment the helper gets: enough for AppKit, nothing else of ours. */
export function helperEnv(env: NodeJS.ProcessEnv = process.env): NodeJS.ProcessEnv {
  const keep = ["HOME", "PATH", "TMPDIR", "USER", "LOGNAME", "LANG", "LC_ALL"];
  return Object.fromEntries(keep.filter((k) => env[k] !== undefined).map((k) => [k, env[k]]));
}

export class NotchProcess {
  private child?: ChildProcess;
  private startedAt = 0;
  private failures = 0;
  private stopping = false;
  private restartTimer?: () => void;
  private readonly backoff: Backoff;
  private readonly spawn: Spawn;
  private readonly after: (ms: number, fire: () => void) => () => void;
  private readonly now: () => number;
  private readonly log: (message: string) => void;

  constructor(private readonly options: NotchProcessOptions) {
    this.backoff = options.backoff ?? DEFAULT_BACKOFF;
    this.spawn = options.spawn ?? nodeSpawn;
    this.after =
      options.after ??
      ((ms, fire) => {
        const t = setTimeout(fire, ms);
        return () => clearTimeout(t);
      });
    this.now = options.now ?? Date.now;
    this.log = options.log ?? ((m) => console.warn(`[cua-spaces] notch: ${m}`));
  }

  /** Whether a helper is running. */
  get running(): boolean {
    return this.child !== undefined;
  }

  /** The helper's pid (tests, logs). */
  get pid(): number | undefined {
    return this.child?.pid;
  }

  start(): void {
    if (this.child || this.stopping) return;
    let child: ChildProcess;
    try {
      child = this.spawn(this.options.path, [], {
        stdio: ["pipe", "pipe", "pipe"],
        env: this.options.env ?? helperEnv(),
      });
    } catch (error) {
      this.log(`could not start ${this.options.path}: ${String(error)}`);
      this.exited(null, null);
      return;
    }
    this.child = child;
    this.startedAt = this.now();
    const lines = new LineSplitter();
    child.stdout?.setEncoding("utf8");
    child.stdout?.on("data", (chunk: string) => {
      for (const line of lines.push(chunk)) {
        const message = parseHelperMessage(line);
        if (message) this.options.onMessage(message);
        else this.log(`ignored a malformed line from the helper (${line.length} bytes)`);
      }
    });
    const errors = new LineSplitter();
    child.stderr?.setEncoding("utf8");
    child.stderr?.on("data", (chunk: string) => {
      for (const line of errors.push(chunk)) this.log(line);
    });
    // The helper gone mid-write: its exit is handled below.
    child.stdin?.on("error", () => {});
    let done = false;
    const finish = (code: number | null, signal: NodeJS.Signals | null) => {
      if (done) return;
      done = true;
      if (this.child === child) this.child = undefined;
      this.exited(code, signal);
    };
    child.on("error", (error) => {
      this.log(`helper error: ${error.message}`);
      finish(null, null);
    });
    child.on("exit", finish);
    this.options.onStart();
  }

  /** One message to the helper (dropped while none runs; a restart resends the state). */
  send(message: HostMessage): void {
    const stdin = this.child?.stdin;
    if (!stdin || stdin.destroyed || !stdin.writable) return;
    stdin.write(encodeMessage(message));
  }

  /** Asks the helper to quit and closes its stdin; kills it if it lingers. */
  stop(): void {
    this.stopping = true;
    this.restartTimer?.();
    this.restartTimer = undefined;
    const child = this.child;
    if (!child) return;
    this.send({ type: "quit" });
    child.stdin?.end();
    const cancel = this.after(2000, () => {
      if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
    });
    child.once("exit", cancel);
  }

  private exited(code: number | null, signal: NodeJS.Signals | null): void {
    if (this.stopping) return;
    if (code === MISMATCH_EXIT) {
      this.log("the helper speaks another notch protocol; not restarting it");
      return;
    }
    const ran = this.now() - this.startedAt;
    this.failures = ran >= this.backoff.stableMs ? 1 : this.failures + 1;
    if (this.failures > this.backoff.maxFailures) {
      this.log(`the helper exited ${this.failures - 1} times in a row; giving up until the app restarts`);
      return;
    }
    const delay = Math.min(this.backoff.maxMs, this.backoff.initialMs * 2 ** (this.failures - 1));
    this.log(`the helper exited (${signal ?? `status ${code}`}); restarting in ${delay} ms`);
    this.restartTimer = this.after(delay, () => {
      this.restartTimer = undefined;
      this.start();
    });
  }
}
