// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// This app's daemon and the launch (the SwiftUI app's DaemonSupervisor and
// StartupModel): the daemon files, the backoff, the ownership rule's paths,
// `cua daemon start` and `cua auth keychain` (on a stand-in `cua`), and the
// launch's states and buttons. Nothing here starts a real daemon or reads
// the keychain.
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import { backoffMs, comparablePath, cuaHome, DaemonSupervisor, daemonPids, executablePath, type SuperviseOptions } from "../src/model/daemon";
import { supervise } from "../src/model/environment";
import type { AppModel } from "../src/model/app-model";
import type { CuaLike } from "../src/native/generated/index";
import { BundledCuaKeychain, parseKeychainCheck, StartupModel, startupState, type KeychainAccessChecking, type KeychainCheckResult } from "../src/model/startup";

const scratch = mkdtempSync(path.join(tmpdir(), "cua-daemon-test-"));
afterAll(() => rmSync(scratch, { recursive: true, force: true }));
const posix = process.platform !== "win32";

/** A stand-in `cua`: records its arguments and environment, prints `out`, exits with `code`. */
function fakeCua(name: string, out: string, code = 0): { bin: string; seen: () => string } {
  const bin = path.join(scratch, name);
  const log = `${bin}.log`;
  writeFileSync(bin, `#!/bin/sh\necho "$@ started_by=$CUA_DAEMON_STARTED_BY quiet=$CUA_KEYCHAIN_NONINTERACTIVE" >> '${log}'\nprintf '%s' '${out}'\nexit ${code}\n`);
  chmodSync(bin, 0o755);
  return { bin, seen: () => readFileSync(log, "utf8") };
}

describe("the daemon's files", () => {
  it("reads the starting and running daemons' pids", () => {
    const files: Record<string, string> = { "daemon.starting": "41\n", "daemon.json": JSON.stringify({ pid: 42, token: "t" }) };
    expect(daemonPids("/h", (f) => files[path.basename(f)] ?? (() => { throw new Error("none"); })())).toEqual([41, 42]);
    expect(daemonPids("/h", () => { throw new Error("none"); })).toEqual([]);
    expect(daemonPids("/h", (f) => (f.endsWith("daemon.json") ? "{\"pid\":7}" : "x"))).toEqual([7]);
  });

  it("finds the cua home in CUA_HOME, else ~/.cua", () => {
    expect(cuaHome({ CUA_HOME: "/x/.cua" })).toBe("/x/.cua");
    expect(cuaHome({ HOME: "/home/ada" })).toBe(path.join("/home/ada", ".cua"));
  });

  it("backs off from 2 s to a minute", () => {
    expect([1, 2, 3, 4, 5, 6, 7].map(backoffMs)).toEqual([2000, 4000, 8000, 16000, 32000, 60000, 60000]);
  });

  it("compares Windows paths with / and in any case", () => {
    const id = (p: string) => p;
    expect(comparablePath("C:\\Users\\Ada\\AppData\\Local\\Programs\\Cua Spaces\\resources\\native\\cua.exe", "win32", id)).toBe(
      "c:/users/ada/appdata/local/programs/cua spaces/resources/native/cua.exe",
    );
    expect(comparablePath("/Applications/Cua Spaces.app/x", "darwin", id)).toBe("/Applications/Cua Spaces.app/x");
  });

  it.skipIf(!posix)("reads a process's executable", async () => {
    expect(await executablePath(process.pid)).toMatch(/node|Electron/i);
    expect(await executablePath(0)).toBeNull();
  });
});

describe.skipIf(!posix)("the supervisor", () => {
  it("runs `cua daemon start` as the app, never letting the daemon prompt", async () => {
    const ok = fakeCua("cua-ok", "started");
    const s = new DaemonSupervisor(ok.bin, scratch, () => false, { PATH: process.env.PATH });
    expect(await s.start()).toBeNull();
    expect(ok.seen()).toContain("daemon start started_by=app quiet=1");
    const bad = fakeCua("cua-bad", "another build holds the socket", 1);
    expect(await new DaemonSupervisor(bad.bin, scratch, () => false, {}).start()).toBe("another build holds the socket");
    expect(await new DaemonSupervisor(path.join(scratch, "missing"), scratch, () => false, {}).start()).toMatch(/could not run/);
  });

  it("asks the core's rule whether a daemon is this app's", async () => {
    const seen: [string | null, string][] = [];
    const s = new DaemonSupervisor("/x/cua", scratch, (exe, bundle) => (seen.push([exe, bundle]), true), {});
    expect(await s.isOwn(process.pid)).toBe(true);
    expect(seen[0]![0]).toMatch(/node|Electron/i);
  });

  it("starts the daemon again when it stops answering, then reconnects", async () => {
    let up = false;
    let starts = 0;
    let reconnects = 0;
    const s = new DaemonSupervisor("/x/cua", scratch, () => false, {});
    const stop = s.supervise({
      intervalMs: 10,
      isUp: async () => up,
      start: async () => {
        starts += 1;
        up = true;
        return null;
      },
      restarted: async () => void (reconnects += 1),
      daemonPid: async () => (up ? process.pid : null),
      report: () => {},
      log: () => {},
    });
    await new Promise((r) => setTimeout(r, 80));
    stop();
    expect(starts).toBe(1);
    expect(reconnects).toBe(1);
  });

  /** A cua home whose `daemon.json` names `pid` (this test process: alive, and never this app's own). */
  const homeWith = (pid: number) => {
    const home = mkdtempSync(path.join(scratch, "home-"));
    writeFileSync(path.join(home, "daemon.json"), JSON.stringify({ pid }));
    return home;
  };

  it("does not reconnect when the start leaves the other app's daemon it already uses", async () => {
    // Another app's daemon of the same or a newer version, which this
    // connection uses, stopped answering; `cua daemon start` kept it. No
    // "started again" reconnect every interval: a failure that backs off.
    let starts = 0;
    let reconnects = 0;
    const errors: (string | null)[] = [];
    const lines: string[] = [];
    const s = new DaemonSupervisor("/x/cua", scratch, () => false, { CUA_HOME: homeWith(process.pid) });
    const stop = s.supervise({
      intervalMs: 10,
      isUp: async () => false,
      start: async () => {
        starts += 1;
        return null;
      },
      restarted: async () => void (reconnects += 1),
      accepted: async () => process.pid,
      daemonPid: async () => null,
      report: (e) => errors.push(e),
      log: (l) => lines.push(l),
    });
    await new Promise((r) => setTimeout(r, 200));
    stop();
    expect(reconnects).toBe(0);
    // The first try after 10 ms, the next after the 2 s backoff.
    expect(starts).toBe(1);
    expect(errors).toEqual([]);
    expect(lines[0]).toMatch(/is another app's, which this app uses, and it does not answer/);
  });

  it("connects once to another app's daemon that the start kept, then stays", async () => {
    // This app's daemon is gone and another app's (not older) runs instead:
    // the start keeps it, the app connects to it once, and it is up.
    let up = false;
    let starts = 0;
    let reconnects = 0;
    const s = new DaemonSupervisor("/x/cua", scratch, () => false, { CUA_HOME: homeWith(process.pid) });
    const stop = s.supervise({
      intervalMs: 10,
      isUp: async () => up,
      start: async () => {
        starts += 1;
        return null;
      },
      restarted: async () => void ((reconnects += 1), (up = true)),
      accepted: async () => 1,
      daemonPid: async () => null,
      report: () => {},
      log: () => {},
    });
    await new Promise((r) => setTimeout(r, 100));
    stop();
    expect([starts, reconnects]).toEqual([1, 1]);
  });
});

describe("the app's supervision", () => {
  it("uses another app's daemon its connection was accepted on, and no other stranger", async () => {
    // `Cua.auto` refuses an older app's daemon, so the one the connection
    // was made on (the same or a newer version) is up; another is not.
    let pid = 42;
    const cua = { info: async () => ({ daemonPid: pid }) } as unknown as CuaLike;
    let options: SuperviseOptions | undefined;
    const supervisor = new DaemonSupervisor("/x/cua", scratch, () => false, {});
    supervisor.supervise = (o) => ((options = o), () => {});
    supervise(cua, supervisor, {} as AppModel);
    expect(await options!.isUp()).toBe(true);
    expect(await options!.accepted!()).toBe(42);
    pid = 43;
    expect(await options!.isUp()).toBe(false);
  });
});

describe("the keychain check", () => {
  it("reads `cua auth keychain`'s answer after anything else it printed", () => {
    expect(parseKeychainCheck('warning\n{"state":"ready"}')).toEqual({ kind: "ready" });
    expect(parseKeychainCheck('{"state":"not_used"}')).toEqual({ kind: "ready" });
    expect(parseKeychainCheck('{"state":"needs_access"}')).toEqual({ kind: "needsAccess", locked: false });
    expect(parseKeychainCheck('{"state":"locked"}')).toEqual({ kind: "needsAccess", locked: true });
    expect(parseKeychainCheck('{"state":"denied"}')).toEqual({ kind: "denied" });
    expect(parseKeychainCheck('{"state":"error","items":[{"error":"vault damaged"}]}')).toEqual({ kind: "failed", why: "vault damaged" });
    expect(parseKeychainCheck("")).toEqual({ kind: "failed", why: "no answer" });
  });

  it.skipIf(!posix)("runs the bundled cua quietly, and only the clicked check may prompt", async () => {
    const cua = fakeCua("cua-keychain", '{"state":"locked"}');
    const k = new BundledCuaKeychain(cua.bin, { PATH: process.env.PATH, CUA_KEYCHAIN_NONINTERACTIVE: "1" });
    expect(await k.check(false)).toEqual({ kind: "needsAccess", locked: true });
    await k.check(true);
    expect(cua.seen().split("\n").filter(Boolean)).toEqual(["auth keychain started_by=app quiet=1", "auth keychain --prompt started_by=app quiet="]);
  });
});

/** A scripted keychain (the SwiftUI app's FixtureKeychain). */
class Keychain implements KeychainAccessChecking {
  calls: string[] = [];
  constructor(
    public quiet: KeychainCheckResult,
    public prompted: KeychainCheckResult = { kind: "ready" },
  ) {}
  async check(prompt: boolean) {
    this.calls.push(prompt ? "prompt" : "check");
    if (!prompt) return this.quiet;
    if (this.prompted.kind === "ready") this.quiet = { kind: "ready" };
    return this.prompted;
  }
  async forget() {
    this.calls.push("forget");
    return { kind: "ready" } as const;
  }
  cancel() {}
}

const settle = () => new Promise((r) => setTimeout(r, 10));

describe("the launch", () => {
  function launch(keychain: Keychain | null) {
    const s = new StartupModel({ kind: "starting" });
    s.keychain = keychain;
    let started = 0;
    let restarted = 0;
    let ready = 0;
    s.start = async () => void (started += 1);
    s.restart = async () => void (restarted += 1);
    s.onReady.push(() => void (ready += 1));
    return { s, counts: () => ({ started, restarted, ready }) };
  }

  it("starts at once when the sign-in can be read", async () => {
    const { s, counts } = launch(new Keychain({ kind: "ready" }));
    const phases: string[] = [];
    s.subscribe(() => phases.push(s.phase.kind));
    s.begin();
    await settle();
    expect(s.isReady).toBe(true);
    expect(counts()).toEqual({ started: 1, restarted: 0, ready: 1 });
    expect(phases[0]).toBe("starting");
    expect(startupState(s)).toEqual({ phase: "ready", slow: false, title: "", body: "", actions: [] });
  });

  it("asks for Keychain access, then starts once it was given", async () => {
    const keychain = new Keychain({ kind: "needsAccess", locked: true });
    const { s, counts } = launch(keychain);
    s.begin();
    await settle();
    expect(startupState(s)).toMatchObject({ phase: "needsKeychain", title: "Unlock your keychain", actions: ["allowAccess", "signInAgain"] });
    expect(counts().started).toBe(0);
    s.act("allowAccess");
    await settle();
    expect(keychain.calls).toEqual(["check", "prompt", "check"]);
    expect(s.isReady).toBe(true);
    expect(counts().started).toBe(1);
  });

  it("says when access was denied, and signs in again on request", async () => {
    const keychain = new Keychain({ kind: "needsAccess", locked: false }, { kind: "denied" });
    const { s, counts } = launch(keychain);
    s.begin();
    await settle();
    s.act("allowAccess");
    await settle();
    expect(startupState(s)).toMatchObject({ phase: "keychainDenied", actions: ["tryAgain", "signInAgain"] });
    s.act("signInAgain");
    await settle();
    expect(keychain.calls.at(-1)).toBe("forget");
    expect(s.isReady).toBe(true);
    expect(counts().started).toBe(1);
  });

  it("gives up waiting on a start that never ends, and Try again restarts the daemon", async () => {
    const { s, counts } = launch(null);
    let release = () => {};
    s.start = () => new Promise<void>((r) => (release = r));
    s.startTimeoutMs = 20;
    s.begin();
    await new Promise((r) => setTimeout(r, 50));
    expect(startupState(s)).toMatchObject({ phase: "startFailed", actions: ["tryAgain"] });
    s.act("tryAgain");
    release();
    await settle();
    expect(counts().restarted).toBe(1);
    expect(s.isReady).toBe(true);
  });

  it("ignores buttons that are not showing", () => {
    const s = new StartupModel({ kind: "ready" });
    s.act("allowAccess");
    s.act("tryAgain");
    expect(s.isReady).toBe(true);
  });
});
