// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// "Join with a code" (`host.joinWithCode`): sets this computer up with a
// one-time pairing code made on another device, by running the bundled
// `cua host setup --pair <code>`, as the install script does. The page
// confirms first; nothing here runs on a link alone. The Swift host answers
// the same method (`JoinWithCode.swift`).
import { spawn } from "node:child_process";

import { string } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";
import { hostStatus } from "./machines";

/** Long enough for the driver download and the service start; the relay answers in seconds. */
export const JOIN_TIMEOUT_MS = 180_000;

const ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/** The 12-character code in what was typed or pasted (`cua_host::normalize_pairing_code`), or null. */
export function normalizeCode(input: string): string | null {
  let s = input.trim();
  const at = s.indexOf("/add/");
  if (at >= 0) s = s.slice(at + 5).split(/[?#/]/)[0] ?? "";
  if (s.startsWith("cpc_")) s = s.slice(4);
  let out = "";
  for (const raw of s) {
    const c = raw.toUpperCase();
    if (c === "-" || c === " ") continue;
    const m = c === "I" || c === "L" ? "1" : c === "O" ? "0" : c;
    if (m.length !== 1 || !ALPHABET.includes(m)) return null;
    out += m;
  }
  return out.length === 12 ? out : null;
}

/** The relay another than the production one that a link names (`?relay=`), when it is an http(s) URL. */
export function linkRelay(input: string): string | null {
  try {
    const v = new URL(input.trim()).searchParams.get("relay")?.trim();
    return v && /^https?:\/\//i.test(v) ? v : null;
  } catch {
    return null;
  }
}

/** `cua`'s arguments for joining with `code` (the canonical code, so nothing typed reaches the command line as is). */
export function joinArgs(code: string, relay: string | null): string[] {
  return ["host", "setup", "--pair", code, ...(relay ? ["--relay", relay] : [])];
}

/** The line to show for a failed run: the last one the command wrote to stderr, else the exit status. */
export function failureText(stderr: string, status: string): string {
  const lines = stderr
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter(Boolean);
  return lines.length ? lines[lines.length - 1]! : `cua host setup ${status}`;
}

export interface CuaRun {
  ok: boolean;
  stderr: string;
  status: string;
}

/** Runs `cua` with `args` and waits for it (stopped after `timeoutMs`). */
export function runCua(cua: string, args: string[], env: NodeJS.ProcessEnv, timeoutMs = JOIN_TIMEOUT_MS): Promise<CuaRun> {
  return new Promise((resolve) => {
    let stderr = "";
    let settled = false;
    const done = (r: CuaRun) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(r);
    };
    let child;
    try {
      child = spawn(cua, args, { env, stdio: ["ignore", "ignore", "pipe"], windowsHide: true });
    } catch (e) {
      resolve({ ok: false, stderr: "", status: `could not start: ${e instanceof Error ? e.message : String(e)}` });
      return;
    }
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      done({ ok: false, stderr, status: "timed out" });
    }, timeoutMs);
    child.stderr?.on("data", (d: Buffer) => {
      stderr = (stderr + d.toString()).slice(-4000);
    });
    child.on("error", (e) => done({ ok: false, stderr: "", status: `could not start: ${e.message}` }));
    child.on("exit", (code, signal) => done({ ok: code === 0, stderr, status: signal ? `stopped by ${signal}` : `exited with ${code}` }));
  });
}

export function joinMethods(ctx: BridgeContext, run: typeof runCua = runCua): Handlers {
  const { model, supervisor, env } = ctx;
  return {
    "host.joinWithCode": async (args) => {
      const input = string(args, "code");
      const code = normalizeCode(input);
      if (!code) throw Failure.badArgs("code: 12 letters and digits, like K7QM-4XPB-2RHD");
      const named = typeof args.relay === "string" && args.relay.trim() ? args.relay.trim() : null;
      if (named && !/^https?:\/\//i.test(named)) throw Failure.badArgs("relay: an http(s) URL");
      if (!supervisor) throw Failure.unsupported("This build has no bundled cua to join with a code. Use the install command on this computer.");
      const result = await run(supervisor.cua, joinArgs(code, named ?? linkRelay(input)), { ...env });
      if (!result.ok) throw Failure.failed(failureText(result.stderr, result.status));
      await model.host.refresh();
      return hostStatus(model);
    },
  };
}
