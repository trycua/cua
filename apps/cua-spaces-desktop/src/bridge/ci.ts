// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The CI Runners page (`ci.*`; the web bridge's ops/ci.ts): each method runs
// the bundled `cua ci app <op>`, with the arguments as one JSON object on
// stdin, and answers the one JSON line it prints
// (`libs/cua/crates/cua-cli/src/ci_app.rs`). The rules (what may be asked,
// what is checked, where a GitHub token goes) are the CLI's; this only
// runs it, with the operation's name fixed here and each argument checked
// for its type. A token reaches `cua` on stdin, never on its command line.
// The SwiftUI host answers the same methods (`WebUIBridge+CI.swift`).
import { spawn } from "node:child_process";

import type { BridgeContext } from "./context";
import { Failure, type BridgeArgs, type Handlers } from "./host";

/** The `cua ci app` operation each method runs. */
export const CI_OPS = {
  "ci.overview": "overview",
  "ci.connect": "connect",
  "ci.connectToken": "connect_token",
  "ci.connectCancel": "connect_cancel",
  "ci.repositories": "repositories",
  "ci.goal": "goal",
  "ci.log": "log",
} as const;

export type CiMethod = keyof typeof CI_OPS;

/** Under the page's 30 s request limit: a slow service answers with a failure the page can show. */
export const CI_TIMEOUT_MS = 25_000;

const str = (v: unknown): string | undefined => (typeof v === "string" && v.trim() !== "" ? v : undefined);
const list = (v: unknown): string[] => (Array.isArray(v) ? v.filter((s): s is string => typeof s === "string" && s.trim() !== "") : []);

/** The JSON the operation takes, from the page's arguments (only its keys, each checked). */
export function ciInput(method: CiMethod, args: BridgeArgs): Record<string, unknown> {
  switch (method) {
    case "ci.overview":
      return { live: args.live === true };
    case "ci.connect": {
      const org = str(args.githubOrg);
      return org ? { githubOrg: org } : {};
    }
    case "ci.connectToken": {
      const org = str(args.org);
      if (!org) throw Failure.badArgs("org: string");
      const token = str(args.token);
      return token ? { org, token } : { org };
    }
    case "ci.connectCancel":
      return {};
    case "ci.repositories":
      return { allow: list(args.allow), remove: list(args.remove) };
    case "ci.goal": {
      const target = str(args.target);
      const goal = str(args.goal);
      if (!target || !goal) throw Failure.badArgs("target and goal: string");
      return { target, goal };
    }
    case "ci.log": {
      const jobKey = str(args.jobKey);
      const pool = str(args.pool);
      if (!jobKey || !pool) throw Failure.badArgs("jobKey and pool: string");
      return { jobKey, pool, slot: str(args.slot) ?? "" };
    }
  }
}

export interface CiRun {
  /** `cua` ran and exited 0. */
  ok: boolean;
  stdout: string;
  stderr: string;
  status: string;
}

/** Runs `cua ci app <op>` with `input` on stdin (stopped after `timeoutMs`). */
export function runCiApp(cua: string, op: string, input: unknown, env: NodeJS.ProcessEnv, timeoutMs = CI_TIMEOUT_MS): Promise<CiRun> {
  return new Promise((resolve) => {
    let stdout = "";
    let stderr = "";
    let settled = false;
    const done = (r: CiRun) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      resolve(r);
    };
    let child;
    try {
      child = spawn(cua, ["ci", "app", op], { env, stdio: ["pipe", "pipe", "pipe"], windowsHide: true });
    } catch (e) {
      resolve({ ok: false, stdout: "", stderr: "", status: `could not start: ${e instanceof Error ? e.message : String(e)}` });
      return;
    }
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      done({ ok: false, stdout, stderr, status: "timed out" });
    }, timeoutMs);
    child.stdout?.on("data", (d: Buffer) => {
      stdout = (stdout + d.toString()).slice(-4_000_000);
    });
    child.stderr?.on("data", (d: Buffer) => {
      stderr = (stderr + d.toString()).slice(-4000);
    });
    child.on("error", (e) => done({ ok: false, stdout, stderr, status: `could not start: ${e.message}` }));
    child.on("exit", (code, signal) => done({ ok: code === 0, stdout, stderr, status: signal ? `stopped by ${signal}` : `exited with ${code}` }));
    // A closed pipe (cua exited early) is cua's own failure to report, not ours.
    child.stdin?.on("error", () => undefined);
    child.stdin?.end(JSON.stringify(input));
  });
}

/** What `cua ci app` printed (its last JSON line): the result, or a failure with its code and sentence. */
export function ciAnswer(run: CiRun): unknown {
  if (!run.ok) {
    const last = run.stderr
      .split(/\r?\n/)
      .map((l) => l.trim())
      .filter(Boolean)
      .pop();
    throw Failure.failed(last ?? `cua ci app ${run.status}`);
  }
  const line = run.stdout
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter(Boolean)
    .pop();
  let v: { ok?: boolean; result?: unknown; error?: { code?: string; message?: string } };
  try {
    v = JSON.parse(line ?? "");
  } catch {
    throw Failure.failed("cua ci app answered something that is not JSON");
  }
  if (v.ok === true) return v.result ?? null;
  const code = v.error?.code;
  const message = v.error?.message ?? "CI Runners could not do that";
  if (code === "invalid") throw Failure.badArgs(message);
  if (code === "not_found") throw Failure.notFound(message);
  if (code === "unsupported") throw Failure.unsupported(message);
  throw Failure.failed(message);
}

export function ciMethods(ctx: BridgeContext, run: typeof runCiApp = runCiApp): Handlers {
  const handlers: Handlers = {};
  for (const method of Object.keys(CI_OPS) as CiMethod[]) {
    handlers[method] = async (args) => {
      const input = ciInput(method, args);
      if (!ctx.supervisor) throw Failure.unsupported("This build has no bundled cua to run CI Runners.");
      return ciAnswer(await run(ctx.supervisor.cua, CI_OPS[method], input, { ...ctx.env }));
    };
  }
  return handlers;
}
