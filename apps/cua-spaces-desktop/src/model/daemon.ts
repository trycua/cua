// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// This app's own `cua daemon` (the SwiftUI app's DaemonSupervisor.swift):
// the bundled `cua`'s build. The Keyvault, the persistent-agent supervisor,
// Cua Volume and host Spaces live only in the daemon, so the app starts it
// at launch, has it replace a daemon of another build, and starts it again
// when it dies. `cua daemon start` does the work: it starts a daemon that
// survives the app, replaces a stranger, and does nothing when this build's
// already runs.
//
// Another app's daemon is replaced at most once per session: an older app
// (an installed release) starts its own again, and replacing it each time
// would only take turns with it. Its executable then goes in
// `CUA_DAEMON_KEEP`, so `cua daemon start` and the SDK keep and use that
// daemon when it comes back, and the app says so (`yieldNotice`).
import { execFile, spawn } from "node:child_process";
import { readFileSync, readlinkSync, realpathSync } from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { sleep } from "./time";

/** `CUA_KEYCHAIN_NONINTERACTIVE` (cua-auth's `KEYCHAIN_NONINTERACTIVE_ENV`). */
export const KEYCHAIN_NONINTERACTIVE_ENV = "CUA_KEYCHAIN_NONINTERACTIVE";

/** `CUA_DAEMON_KEEP` (cua-daemon's `identity::KEEP_ENV`): other apps' daemon executables this app replaced once. */
export const DAEMON_KEEP_ENV = "CUA_DAEMON_KEEP";

/** The cua home the daemon uses (`CUA_HOME`, else `~/.cua`). */
export function cuaHome(env: NodeJS.ProcessEnv = process.env): string {
  if (env.CUA_HOME) return env.CUA_HOME;
  return path.join(env.HOME || env.USERPROFILE || os.homedir(), ".cua");
}

/** The daemon still starting (`daemon.starting`) and the one running (`daemon.json`), as their files name them. */
export function daemonPids(home: string, read: (file: string) => string = (f) => readFileSync(f, "utf8")): number[] {
  const out = new Set<number>();
  try {
    const pid = Number.parseInt(read(path.join(home, "daemon.starting")).trim(), 10);
    if (pid > 0) out.add(pid);
  } catch {
    // None starting.
  }
  try {
    const pid = (JSON.parse(read(path.join(home, "daemon.json"))) as { pid?: unknown }).pid;
    if (typeof pid === "number" && pid > 0) out.add(pid);
  } catch {
    // None running.
  }
  return [...out].sort((a, b) => a - b);
}

/** The running daemon's pid (`daemon.json`), when its file names one. */
export function runningPid(home: string, read: (file: string) => string = (f) => readFileSync(f, "utf8")): number | null {
  try {
    const pid = (JSON.parse(read(path.join(home, "daemon.json"))) as { pid?: unknown }).pid;
    return typeof pid === "number" && pid > 0 ? pid : null;
  } catch {
    return null;
  }
}

/** The app a daemon executable belongs to (its `.app` bundle on macOS), else the executable. */
export function appOf(executable: string): string {
  const m = /^(.*?\.app)[\\/]Contents[\\/]/i.exec(executable);
  return m ? m[1]! : executable;
}

/** What the app says while it uses another app's daemon it yields to (that app keeps starting its own). */
export function yieldNotice(executable: string, version: string | undefined): string {
  const which = version ? `${appOf(executable)}, cua ${version}` : appOf(executable);
  return (
    `Another Cua Spaces app (${which}) is running and keeps starting its own daemon, so this app uses that one. ` +
    "Quit the other app, then reopen Cua Spaces to use all of this one's features."
  );
}

/** The wait before try `n` (1, 2, ...): 2 s doubling, at most a minute (ms). */
export function backoffMs(n: number): number {
  return Math.min(60, 2 << Math.min(Math.max(n - 1, 0), 5)) * 1000;
}

const alive = (pid: number) => {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return (error as NodeJS.ErrnoException).code === "EPERM";
  }
};

/** The executable a process runs, when the system says. */
export function executablePath(pid: number, platform: NodeJS.Platform = process.platform): Promise<string | null> {
  if (pid <= 0) return Promise.resolve(null);
  if (platform === "linux") {
    try {
      return Promise.resolve(readlinkSync(`/proc/${pid}/exe`));
    } catch {
      return Promise.resolve(null);
    }
  }
  const [cmd, args] =
    platform === "win32"
      ? ["powershell.exe", ["-NoProfile", "-NonInteractive", "-Command", `(Get-Process -Id ${pid}).Path`]]
      : ["ps", ["-o", "comm=", "-p", String(pid)]];
  return new Promise((resolve) => {
    execFile(cmd as string, args as string[], { timeout: 5000, windowsHide: true }, (error, stdout) => {
      const out = String(stdout ?? "").trim();
      resolve(error || !out ? null : out);
    });
  });
}

/** A path as the core's ownership rule compares it: real, with `/` separators (and, on Windows, any case). */
export function comparablePath(p: string, platform: NodeJS.Platform = process.platform, real: (p: string) => string = realpathSync): string {
  let out = p;
  try {
    out = real(p);
  } catch {
    // Gone or unreadable: compare it as given.
  }
  if (platform === "win32") out = out.replace(/\\/g, "/").toLowerCase();
  return out;
}

export interface SuperviseOptions {
  intervalMs?: number;
  /** Whether the daemon this app uses answers (its own, or another app's it was accepted on; an older build's does not count). */
  isUp: () => Promise<boolean>;
  /** Why it could not be started (after three tries in a row), and null once it answers again. */
  report: (error: string | null) => void;
  /** After each start that left this app's daemon running (the app connects again). */
  restarted?: () => Promise<void>;
  /** Replaces `start` (tests). */
  start?: () => Promise<string | null>;
  /** The daemon's pid to watch (by default the one in `daemon.json`, when it is this app's). */
  daemonPid?: () => Promise<number | null>;
  /**
   * The daemon this connection uses when it is another app's (kept because
   * it is not older than this app's cua): a start that leaves exactly that
   * daemon running does not connect again.
   */
  accepted?: () => Promise<number | undefined>;
  log?: (line: string) => void;
}

export class DaemonSupervisor {
  /** Other apps' daemon executables (as `comparablePath` has them) this app replaced, once each. */
  private readonly replaced = new Set<string>();

  constructor(
    /** The bundled `cua`. */
    readonly cua: string,
    /** The directory the bundled `cua` lives in: a daemon whose executable is under it is this app's. */
    readonly bundle: string,
    private readonly ownership: (daemonExe: string | null, bundle: string) => boolean,
    private readonly env: NodeJS.ProcessEnv = process.env,
    private readonly platform: NodeJS.Platform = process.platform,
  ) {}

  get cuaHome(): string {
    return cuaHome(this.env);
  }

  /** Whether the daemon `pid` is this app's own: its executable is in this app's native directory (the core's rule). */
  async isOwn(pid: number): Promise<boolean> {
    const exe = await executablePath(pid, this.platform);
    return this.ownership(exe && comparablePath(exe, this.platform), comparablePath(this.bundle, this.platform));
  }

  /** The running daemon when it is another app's: its pid and executable (as `comparablePath` has it). */
  async stranger(): Promise<{ pid: number; executable: string } | null> {
    const pid = runningPid(this.cuaHome);
    if (pid === null || !alive(pid)) return null;
    const exe = await executablePath(pid, this.platform);
    if (!exe) return null;
    const executable = comparablePath(exe, this.platform);
    if (this.ownership(executable, comparablePath(this.bundle, this.platform))) return null;
    return { pid, executable };
  }

  /** The executable of the daemon `pid` when it is another app's that this app replaced once and now yields to. */
  async yieldsTo(pid: number): Promise<string | null> {
    if (this.replaced.size === 0) return null;
    const exe = await executablePath(pid, this.platform);
    if (!exe) return null;
    const executable = comparablePath(exe, this.platform);
    return this.replaced.has(executable) ? exe : null;
  }

  /**
   * `cua daemon start`, bounded: null when this app's daemon runs
   * afterwards (or another app's it keeps), else why not. Another app's
   * daemon it replaced is remembered, so that daemon is kept when its app
   * starts it again (`CUA_DAEMON_KEEP`, here and for the SDK in this process).
   */
  async start(timeoutMs = 60_000): Promise<string | null> {
    const before = await this.stranger();
    const error = await this.run(["daemon", "start"], timeoutMs);
    // Replaced: another daemon is named now, or that one is gone.
    const replaced = before !== null && (runningPid(this.cuaHome) !== before.pid || !alive(before.pid));
    if (error === null && before && replaced && !this.replaced.has(before.executable)) {
      this.replaced.add(before.executable);
      this.env[DAEMON_KEEP_ENV] = [...this.replaced].join(this.platform === "win32" ? ";" : ":");
      console.log(`[cua-spaces] replaced another app's cua daemon (${before.executable}); if it comes back, it is kept`);
    }
    return error;
  }

  /** Stops this app's daemon (bounded), kills one of this bundle that still starts or no longer answers, then starts it. */
  async restart(timeoutMs = 30_000): Promise<string | null> {
    await this.run(["daemon", "stop"], 15_000);
    for (const pid of daemonPids(this.cuaHome)) {
      if (await this.isOwn(pid)) await DaemonSupervisor.stop(pid);
    }
    return this.start(timeoutMs);
  }

  /** SIGTERM, then SIGKILL when it is still there after `graceMs`. */
  static async stop(pid: number, graceMs = 2000): Promise<void> {
    try {
      process.kill(pid, "SIGTERM");
    } catch {
      return;
    }
    const deadline = Date.now() + graceMs;
    while (alive(pid) && Date.now() < deadline) await sleep(50);
    if (alive(pid)) {
      try {
        process.kill(pid, "SIGKILL");
      } catch {
        // Gone.
      }
    }
  }

  private run(args: string[], timeoutMs: number): Promise<string | null> {
    return new Promise((resolve) => {
      const child = spawn(this.cua, args, {
        env: {
          ...this.env,
          CUA_DAEMON_STARTED_BY: "app",
          // The daemon never waits on a keychain prompt nobody may see; the app asks itself.
          [KEYCHAIN_NONINTERACTIVE_ENV]: "1",
        },
        stdio: ["ignore", "pipe", "pipe"],
        windowsHide: true,
        // The daemon outlives this command, and this app.
        detached: this.platform !== "win32",
      });
      let output = "";
      child.stdout?.on("data", (d: Buffer) => (output += d.toString()));
      child.stderr?.on("data", (d: Buffer) => (output += d.toString()));
      const timer = setTimeout(() => {
        child.kill();
        resolve(`\`cua ${args.join(" ")}\` did not finish in ${Math.round(timeoutMs / 1000)} s`);
      }, timeoutMs);
      child.on("error", (error) => {
        clearTimeout(timer);
        resolve(`could not run ${this.cua}: ${error.message}`);
      });
      // On exit, not close: the daemon it starts may hold the pipes open.
      child.on("exit", (code) => {
        clearTimeout(timer);
        const text = output.trim();
        if (code === 0) {
          if (text) console.log(`[cua-spaces] ${text}`);
          resolve(null);
        } else {
          resolve(text || `\`cua ${args.join(" ")}\` exited with ${code}`);
        }
      });
    });
  }

  /**
   * Every `intervalMs`, `isUp` says whether this app's daemon answers; when
   * it does not, `start` runs again, backing off from 2 s to a minute. In
   * between it watches the daemon's process, so a killed daemon is started
   * again in about a second. Returns the stop.
   */
  supervise(o: SuperviseOptions): () => void {
    const intervalMs = o.intervalMs ?? 10_000;
    const log = o.log ?? ((line) => console.log(`[cua-spaces] ${line}`));
    let cancelled = false;
    let wake: (() => void) | null = null;
    const pidNow =
      o.daemonPid ??
      (async () => {
        for (const pid of daemonPids(this.cuaHome)) if (await this.isOwn(pid)) return pid;
        return null;
      });
    /** Returns after `ms`, or as soon as the watched daemon exits. */
    const wait = (ms: number, pid: number | null) =>
      new Promise<void>((resolve) => {
        const done = () => {
          clearTimeout(timer);
          clearInterval(watch);
          wake = null;
          resolve();
        };
        wake = done;
        const timer = setTimeout(done, ms);
        const watch = setInterval(() => {
          if (pid !== null && !alive(pid)) done();
        }, 1000);
        if (pid !== null && !alive(pid)) done();
      });
    void (async () => {
      let failures = 0;
      let reported = false;
      while (!cancelled) {
        const pid = failures === 0 ? await pidNow() : null;
        await wait(failures === 0 ? intervalMs : backoffMs(failures), pid);
        if (cancelled) return;
        if (await o.isUp()) {
          if (reported) o.report(null);
          failures = 0;
          reported = false;
          continue;
        }
        let error = o.start ? await o.start() : await this.start();
        // `cua daemon start` succeeds when a daemon already runs, and keeps
        // another app's of the same or a newer version. When that is the
        // daemon this connection already uses, connecting again would only
        // find it again, every interval: it is a daemon that does not answer.
        if (error === null && o.accepted) {
          const running = daemonPids(this.cuaHome).at(-1);
          if (running !== undefined && running === (await o.accepted()) && !(await this.isOwn(running))) {
            error = `the running cua daemon (pid ${running}) is another app's, which this app uses, and it does not answer`;
          }
        }
        if (error === null) {
          // Running now: connect again, which supervises the new connection.
          if (o.restarted) {
            await o.restarted();
            if (cancelled) return;
          }
          if (await o.isUp()) {
            if (reported) o.report(null);
            failures = 0;
            reported = false;
            continue;
          }
        }
        failures += 1;
        log(`the cua daemon is not answering (try ${failures}): ${error ?? "no answer"}`);
        if (failures >= 3 && !reported) {
          reported = true;
          o.report(
            `The Cua daemon stopped and could not be started again: ${error ?? "it does not answer"}. ` +
              "The Keyvault, agents and Cua Volume need it; Cua Spaces keeps trying.",
          );
        }
      }
    })();
    return () => {
      cancelled = true;
      (wake as (() => void) | null)?.();
    };
  }
}
