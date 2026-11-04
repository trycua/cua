// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The launch (the SwiftUI app's AppStartup.swift). The window opens first;
// `StartupModel` then checks, without any prompt, that the saved sign-in can
// be read (`cua auth keychain`), asks the user to allow access when it
// cannot (the one macOS prompt, only after a click), starts this app's
// daemon and the SDK, and hands the live services over. Every step shows on
// the page's startup screen (`startup.get`, `startup.changed`).
import { spawn, type ChildProcess } from "node:child_process";
import { KEYCHAIN_NONINTERACTIVE_ENV } from "./daemon";
import { sleep } from "./time";

/** What the keychain check found. */
export type KeychainCheckResult =
  | { kind: "ready" }
  | { kind: "needsAccess"; locked: boolean }
  | { kind: "denied" }
  | { kind: "failed"; why: string };

export interface KeychainAccessChecking {
  /** Whether the Cua items can be read; with `prompt`, macOS asks where needed. */
  check(prompt: boolean): Promise<KeychainCheckResult>;
  /** Removes the saved sign-in without reading it ("Sign in again"). */
  forget(): Promise<KeychainCheckResult>;
  /** Stops a check still running (its prompt goes away with it). */
  cancel(): void;
}

/** `cua auth keychain`'s JSON (`cua_auth::KeychainCheck`), after anything else it printed. */
export function parseKeychainCheck(text: string): KeychainCheckResult {
  const start = text.indexOf("{");
  const end = text.lastIndexOf("}");
  let obj: { state?: unknown; items?: { error?: unknown }[] } | null = null;
  try {
    obj = start >= 0 && end > start ? JSON.parse(text.slice(start, end + 1)) : null;
  } catch {
    obj = null;
  }
  if (!obj || typeof obj.state !== "string") return { kind: "failed", why: text ? text.slice(0, 200) : "no answer" };
  switch (obj.state) {
    case "ready":
    case "not_used":
      return { kind: "ready" };
    case "needs_access":
      return { kind: "needsAccess", locked: false };
    case "locked":
      return { kind: "needsAccess", locked: true };
    case "denied":
      return { kind: "denied" };
    default: {
      const why = (obj.items ?? []).map((i) => i.error).find((e): e is string => typeof e === "string");
      return { kind: "failed", why: why ?? obj.state };
    }
  }
}

/** The bundled `cua`'s `auth keychain`: a process of its own, so a prompt still up goes away when the check is cancelled. */
export class BundledCuaKeychain implements KeychainAccessChecking {
  private running: ChildProcess | null = null;

  constructor(
    readonly cua: string,
    private readonly env: NodeJS.ProcessEnv = process.env,
    private readonly quietTimeoutMs = 15_000,
    private readonly killAfterMs = 2_000,
  ) {}

  check(prompt: boolean): Promise<KeychainCheckResult> {
    return this.run(prompt ? ["auth", "keychain", "--prompt"] : ["auth", "keychain"], prompt ? null : this.quietTimeoutMs, prompt);
  }

  forget(): Promise<KeychainCheckResult> {
    return this.run(["auth", "keychain", "--forget"], this.quietTimeoutMs, false);
  }

  cancel(): void {
    const p = this.running;
    if (p && p.exitCode === null) this.stop(p);
  }

  private stop(p: ChildProcess): void {
    p.kill("SIGTERM");
    setTimeout(() => {
      if (p.exitCode === null && p.signalCode === null) p.kill("SIGKILL");
    }, this.killAfterMs).unref();
  }

  /** Waits (bounded) until the previous check has exited, so a new prompt never queues behind one still open. */
  private async stopPrevious(): Promise<void> {
    const p = this.running;
    if (!p || p.exitCode !== null || p.signalCode !== null) return;
    this.stop(p);
    const deadline = Date.now() + this.killAfterMs + 3000;
    while (p.exitCode === null && p.signalCode === null && Date.now() < deadline) await sleep(50);
  }

  private async run(args: string[], timeoutMs: number | null, interactive: boolean): Promise<KeychainCheckResult> {
    await this.stopPrevious();
    const env: NodeJS.ProcessEnv = { ...this.env, CUA_DAEMON_STARTED_BY: "app" };
    // Only the explicit, clicked prompt may show one.
    if (interactive) delete env[KEYCHAIN_NONINTERACTIVE_ENV];
    else env[KEYCHAIN_NONINTERACTIVE_ENV] = "1";
    return new Promise((resolve) => {
      let child: ChildProcess;
      try {
        child = spawn(this.cua, args, { env, stdio: ["ignore", "pipe", "ignore"], windowsHide: true });
      } catch (error) {
        resolve({ kind: "failed", why: `could not run ${this.cua}: ${String(error)}` });
        return;
      }
      this.running = child;
      let out = "";
      child.stdout?.on("data", (d: Buffer) => (out += d.toString()));
      const timer = timeoutMs === null ? null : setTimeout(() => this.stop(child), timeoutMs);
      child.on("error", (error) => {
        if (timer) clearTimeout(timer);
        resolve({ kind: "failed", why: `could not run ${this.cua}: ${error.message}` });
      });
      child.on("exit", (_code, signal) => {
        if (timer) clearTimeout(timer);
        if (this.running === child) this.running = null;
        // Stopped or killed: never an answer (least of all access).
        resolve(signal ? { kind: "failed", why: "stopped" } : parseKeychainCheck(out));
      });
    });
  }
}

export type StartupPhase =
  | { kind: "starting" }
  | { kind: "needsKeychain"; locked: boolean }
  | { kind: "waitingForKeychain" }
  | { kind: "keychainDenied" }
  | { kind: "startFailed" }
  | { kind: "ready" };

export type StartupAction = "allowAccess" | "tryAgain" | "signInAgain";
export const STARTUP_ACTIONS: readonly StartupAction[] = ["allowAccess", "tryAgain", "signInAgain"];

export interface StartupCopy {
  title: string;
  body: string;
  /** The buttons, the primary one first. */
  actions: StartupAction[];
}

const NOT_GIVEN = "macOS didn't give access. Click Allow access to ask again.";
const STILL_WAITING = "Cua is waiting for Keychain access.";

export class StartupModel {
  phase: StartupPhase;
  /** Waiting longer than expected. */
  slow = false;
  /** The prompt we asked for is no longer on screen. */
  promptHidden = false;
  /** Why the window asks again. */
  note: string | null = null;

  keychain: KeychainAccessChecking | null = null;
  /** Starts the daemon and the SDK and hands them over. */
  start: (() => Promise<void>) | null = null;
  /** Restarts this app's daemon (Try again, and access given after the services were in). */
  restart: (() => Promise<void>) | null = null;
  onReady: (() => void)[] = [];
  /** Whether the keychain prompt is on screen (null: unknown). */
  promptShowing: () => boolean | null = () => null;
  slowAfterMs = 25_000;
  hiddenAfterMs = 25_000;
  startingSlowAfterMs = 20_000;
  /** "Starting Cua…" never lasts longer than this. */
  startTimeoutMs = 75_000;

  private attempt = 0;
  private servicesIn = false;
  private readonly listeners = new Set<() => void>();

  constructor(phase: StartupPhase = { kind: "ready" }) {
    this.phase = phase;
  }

  get isReady(): boolean {
    return this.phase.kind === "ready";
  }

  /** Called whenever what the page shows changes; returns the unsubscribe. */
  subscribe(listener: () => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private changed(): void {
    for (const l of [...this.listeners]) l();
  }

  /** Begins the launch: the quiet keychain check, then the start. */
  begin(): void {
    const current = ++this.attempt;
    this.show({ kind: "starting" }, null);
    void this.checkThenLaunch(current, null);
  }

  /** A button on the page. */
  act(action: StartupAction): void {
    const k = this.phase.kind;
    const asking = k === "needsKeychain" || k === "waitingForKeychain" || k === "keychainDenied";
    if (action === "signInAgain" && asking) void this.forget();
    else if (action === "tryAgain" && k === "startFailed") void this.retryStart();
    else if ((action === "allowAccess" || action === "tryAgain") && asking) void this.ask();
  }

  get copy(): StartupCopy {
    switch (this.phase.kind) {
      case "ready":
        return { title: "", body: "", actions: [] };
      case "starting":
        return this.slow
          ? { title: "Still starting Cua…", body: "This can take a minute after an update.", actions: [] }
          : { title: "Starting Cua…", body: "", actions: [] };
      case "needsKeychain": {
        const locked = this.phase.locked;
        const base = locked
          ? "Cua Spaces keeps your sign-in in the macOS Keychain, which is locked. Click Allow access, then enter your Mac login password."
          : "Cua Spaces keeps your sign-in in the macOS Keychain. This version needs your permission to read it. Click Allow access, then enter your Mac login password and choose Always Allow.";
        return {
          title: locked ? "Unlock your keychain" : "Allow Keychain access",
          body: this.note ? `${this.note} ${base}` : base,
          actions: ["allowAccess", "signInAgain"],
        };
      }
      case "waitingForKeychain": {
        if (this.promptHidden) {
          return {
            title: "The password prompt isn't showing",
            body: "The macOS password prompt was closed or is hidden. Click Try again to show it again.",
            actions: ["tryAgain", "signInAgain"],
          };
        }
        const body =
          "macOS is asking for your login password so Cua Spaces can read your sign-in. " +
          "Look for the prompt (it can be behind other windows). Enter your password and choose Always Allow.";
        return this.slow
          ? { title: "Still waiting for Keychain access", body, actions: ["tryAgain", "signInAgain"] }
          : { title: "Waiting for Keychain access…", body, actions: [] };
      }
      case "keychainDenied":
        return {
          title: "Keychain access was not allowed",
          body: "Cua Spaces can't read your sign-in without it. Try again and choose Always Allow, or sign in again.",
          actions: ["tryAgain", "signInAgain"],
        };
      case "startFailed":
        return {
          title: "Cua's background service didn't start",
          body: "Cua Spaces couldn't start its background service. Click Try again to start it again.",
          actions: ["tryAgain"],
        };
    }
  }

  private async checkThenLaunch(current: number, note: string | null): Promise<void> {
    if (this.keychain) {
      const result = await this.keychain.check(false);
      if (current !== this.attempt) return;
      if (result.kind === "needsAccess") return this.show({ kind: "needsKeychain", locked: result.locked }, note);
      if (result.kind === "denied") return this.show({ kind: "needsKeychain", locked: false }, note);
      // A check that cannot run never blocks the launch.
      if (result.kind === "failed") console.log(`[cua-spaces] the keychain check failed (${result.why}); starting anyway`);
    }
    await this.launch(current);
  }

  private async ask(): Promise<void> {
    const current = ++this.attempt;
    if (!this.keychain) return this.launch(current);
    this.keychain.cancel();
    this.show({ kind: "waitingForKeychain" }, null);
    this.markSlow(this.slowAfterMs, current);
    this.watchPrompt(current);
    const result = await this.keychain.check(true);
    if (current !== this.attempt) return;
    if (result.kind === "denied") return this.show({ kind: "keychainDenied" }, null);
    // Only a fresh check without a prompt says whether access was given.
    await this.checkThenLaunch(current, NOT_GIVEN);
  }

  private async forget(): Promise<void> {
    const current = ++this.attempt;
    this.keychain?.cancel();
    this.show({ kind: "starting" }, null);
    const result = await this.keychain?.forget();
    if (current !== this.attempt) return;
    if (result?.kind === "needsAccess") console.log("[cua-spaces] after Sign in again a Cua keychain item still needs access");
    await this.launch(current);
  }

  private async retryStart(): Promise<void> {
    const current = ++this.attempt;
    this.show({ kind: "starting" }, null);
    await this.restart?.();
    if (current !== this.attempt) return;
    await this.checkThenLaunch(current, STILL_WAITING);
  }

  private async launch(current: number): Promise<void> {
    this.show({ kind: "starting" }, null);
    this.markSlow(this.startingSlowAfterMs, current);
    if (this.servicesIn) {
      // Access came after the services were in: the daemon reads the session again.
      await this.restart?.();
    } else {
      this.watchStart(current);
      await this.start?.();
      this.servicesIn = true;
    }
    const k = this.phase.kind;
    if (current !== this.attempt && k !== "startFailed" && k !== "starting") return;
    this.attempt += 1;
    this.show({ kind: "ready" }, null);
    const ready = this.onReady;
    this.onReady = [];
    for (const f of ready) f();
  }

  private watchStart(current: number): void {
    setTimeout(async () => {
      if (this.attempt !== current || this.phase.kind !== "starting") return;
      const diagnosis = ++this.attempt;
      const check = this.keychain ? await this.keychain.check(false) : null;
      if (this.attempt !== diagnosis || this.phase.kind !== "starting") return;
      if (check?.kind === "needsAccess") this.show({ kind: "needsKeychain", locked: check.locked }, STILL_WAITING);
      else this.show({ kind: "startFailed" }, null);
    }, this.startTimeoutMs).unref?.();
  }

  private watchPrompt(current: number): void {
    setTimeout(async () => {
      while (this.attempt === current && this.phase.kind === "waitingForKeychain") {
        const showing = this.promptShowing();
        if (showing !== null && this.promptHidden !== !showing) {
          this.promptHidden = !showing;
          this.changed();
        }
        await sleep(2000);
      }
    }, this.hiddenAfterMs).unref?.();
  }

  private show(phase: StartupPhase, note: string | null): void {
    this.phase = phase;
    this.note = note;
    this.slow = false;
    this.promptHidden = false;
    this.changed();
  }

  private markSlow(afterMs: number, current: number): void {
    setTimeout(() => {
      const k = this.phase.kind;
      if (this.attempt !== current || (k !== "starting" && k !== "waitingForKeychain")) return;
      this.slow = true;
      this.changed();
    }, afterMs).unref?.();
  }
}

/** The page's `StartupState` (`ops/startup.ts` in the web bridge). */
export function startupState(s: StartupModel) {
  const copy = s.copy;
  return { phase: s.phase.kind, slow: s.slow, title: copy.title, body: copy.body, actions: copy.actions };
}
