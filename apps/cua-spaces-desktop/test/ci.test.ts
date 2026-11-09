// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The CI Runners methods (`ci.*`): each runs the bundled `cua ci app <op>` with
// its JSON on stdin and answers the one JSON line it prints.
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import { CI_OPS, ciAnswer, ciInput, ciMethods, runCiApp, type CiRun } from "../src/bridge/ci";
import type { BridgeContext } from "../src/bridge/context";
import { Failure } from "../src/bridge/host";

const ok = (v: unknown): CiRun => ({ ok: true, stdout: `${JSON.stringify({ ok: true, result: v })}\n`, stderr: "", status: "exited with 0" });

describe("what each method sends", () => {
  it("fixes the operation's name and keeps only its own keys", () => {
    expect(CI_OPS["ci.connectToken"]).toBe("connect_token");
    expect(ciInput("ci.overview", { live: true, extra: "x" })).toEqual({ live: true });
    expect(ciInput("ci.overview", {})).toEqual({ live: false });
    expect(ciInput("ci.connect", { githubOrg: " acme ", other: 1 })).toEqual({ githubOrg: " acme " });
    expect(ciInput("ci.connect", {})).toEqual({});
    expect(ciInput("ci.repositories", { allow: ["a/b", 3, ""], remove: "no" })).toEqual({ allow: ["a/b"], remove: [] });
    expect(ciInput("ci.goal", { target: "aws", goal: "fastest", x: 1 })).toEqual({ target: "aws", goal: "fastest" });
    expect(ciInput("ci.log", { jobKey: "1/2", pool: "hosted" })).toEqual({ jobKey: "1/2", pool: "hosted", slot: "" });
  });
  it("refuses what is missing", () => {
    expect(() => ciInput("ci.connectToken", {})).toThrow(Failure);
    expect(() => ciInput("ci.goal", { target: "aws" })).toThrow(Failure);
    expect(() => ciInput("ci.log", { jobKey: "1/2" })).toThrow(Failure);
  });
});

describe("what comes back", () => {
  it("is the result of the last JSON line", () => {
    expect(ciAnswer(ok({ a: 1 }))).toEqual({ a: 1 });
    expect(ciAnswer({ ...ok(null), stdout: `noise\n${ok({ b: 2 }).stdout}` })).toEqual({ b: 2 });
  });
  it("is cua's own failure, with its code mapped", () => {
    const fail = (code: string) => ({ ok: true, stdout: `${JSON.stringify({ ok: false, error: { code, message: "words" } })}\n`, stderr: "", status: "" });
    for (const [code, want] of [
      ["invalid", "bad_args"],
      ["not_found", "not_found"],
      ["unsupported", "unsupported"],
      ["service", "failed"],
    ] as const) {
      expect(() => ciAnswer(fail(code))).toThrowError(expect.objectContaining({ code: want, message: "words" }));
    }
  });
  it("says what a cua that crashed wrote last, and refuses output that is not JSON", () => {
    expect(() => ciAnswer({ ok: false, stdout: "", stderr: "x\nerror: no home\n", status: "exited with 2" })).toThrowError(
      expect.objectContaining({ message: "error: no home" }),
    );
    expect(() => ciAnswer({ ok: true, stdout: "hello", stderr: "", status: "" })).toThrow(/not JSON/);
  });
});

describe.skipIf(process.platform === "win32")("running cua ci app", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "ci-cua-"));
  afterAll(() => rmSync(dir, { recursive: true, force: true }));
  const script = (name: string, body: string) => {
    const file = path.join(dir, name);
    writeFileSync(file, `#!/bin/sh\n${body}\n`);
    chmodSync(file, 0o755);
    return file;
  };
  it("passes the operation as arguments and the JSON on stdin, never the token on the command line", async () => {
    const argv = path.join(dir, "argv.txt");
    const stdin = path.join(dir, "stdin.txt");
    const cua = script("echo", `printf '%s\\n' "$@" > "${argv}"; cat > "${stdin}"; echo '{"ok":true,"result":{"state":"connected"}}'`);
    const r = await runCiApp(cua, "connect_token", { org: "acme", token: "github_pat_SECRET" }, {});
    expect(r.ok).toBe(true);
    expect(ciAnswer(r)).toEqual({ state: "connected" });
    expect(readFileSync(argv, "utf8").trim().split("\n")).toEqual(["ci", "app", "connect_token"]);
    expect(readFileSync(argv, "utf8")).not.toContain("SECRET");
    expect(JSON.parse(readFileSync(stdin, "utf8"))).toEqual({ org: "acme", token: "github_pat_SECRET" });
  });
  it("stops a run that takes too long and reports a cua that cannot start", async () => {
    expect(await runCiApp(script("slow", "sleep 5"), "overview", {}, {}, 100)).toMatchObject({ ok: false, status: "timed out" });
    expect((await runCiApp(path.join(dir, "missing"), "overview", {}, {})).ok).toBe(false);
  });
});

describe("the methods", () => {
  const make = (supervisor: unknown = { cua: "/app/cua" }) => {
    const calls: { cua: string; op: string; input: unknown }[] = [];
    const ctx = { supervisor, env: { HOME: "/h" } } as unknown as BridgeContext;
    const handlers = ciMethods(ctx, async (cua, op, input) => (calls.push({ cua, op, input }), ok({ done: op })));
    const call = (m: string, args: Record<string, unknown>) => Promise.resolve(handlers[m]!(args as never, null as never));
    return { call, calls, handlers };
  };
  it("answers every ci method the page calls", () => {
    expect(Object.keys(make().handlers).sort()).toEqual(Object.keys(CI_OPS).sort());
  });
  it("runs the bundled cua with the fixed operation", async () => {
    const t = make();
    expect(await t.call("ci.overview", { live: true })).toEqual({ done: "overview" });
    expect(await t.call("ci.goal", { target: "aws", goal: "cheapest" })).toEqual({ done: "goal" });
    expect(t.calls).toEqual([
      { cua: "/app/cua", op: "overview", input: { live: true } },
      { cua: "/app/cua", op: "goal", input: { target: "aws", goal: "cheapest" } },
    ]);
  });
  it("checks arguments before running anything, and is unsupported without a bundled cua", async () => {
    const t = make();
    await expect(t.call("ci.connectToken", {})).rejects.toMatchObject({ code: "bad_args" });
    expect(t.calls).toEqual([]);
    await expect(make(null).call("ci.overview", {})).rejects.toMatchObject({ code: "unsupported" });
  });
});
