// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// "Join with a code" (`host.joinWithCode`) and the `cua://add/<code>` link
// (the SwiftUI app's JoinWithCodeTests, ported).
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import * as path from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import type { BridgeContext } from "../src/bridge/context";
import { Failure } from "../src/bridge/host";
import { failureText, joinArgs, joinMethods, linkRelay, normalizeCode, runCua, type CuaRun } from "../src/bridge/join-with-code";
import { deepLinkInArgv, routeForDeepLink } from "../src/deep-link";

describe("pairing codes", () => {
  it("reads any case, with or without dashes, I/L as 1 and O as 0, and an add-machine link", () => {
    expect(normalizeCode("k7qm-4xpb-2rhd")).toBe("K7QM4XPB2RHD");
    expect(normalizeCode(" K7QM 4XPB 2RHD ")).toBe("K7QM4XPB2RHD");
    expect(normalizeCode("IL0O-IL0O-IL0O")).toBe("110011001100");
    expect(normalizeCode("https://cua.ai/add/K7QM-4XPB-2RHD?relay=https%3A%2F%2Fr.example")).toBe("K7QM4XPB2RHD");
  });
  it("rejects what is not a code", () => {
    for (const bad of ["", "K7QM-4XPB", "K7QM-4XPB-2RHD-9", "U7QM-4XPB-2RHD", "K7QM-4XPB-2RH!", "; rm -rf ~"]) expect(normalizeCode(bad)).toBeNull();
  });
  it("finds the relay a link names, http(s) only", () => {
    expect(linkRelay("https://cua.ai/add/K7QM-4XPB-2RHD?relay=https%3A%2F%2Fr.example")).toBe("https://r.example");
    expect(linkRelay("https://cua.ai/add/K7QM-4XPB-2RHD?relay=file%3A%2F%2F%2Fetc")).toBeNull();
    expect(linkRelay("K7QM-4XPB-2RHD")).toBeNull();
  });
});

describe("the command", () => {
  it("passes the canonical code, and the relay when one is named", () => {
    expect(joinArgs("K7QM4XPB2RHD", null)).toEqual(["host", "setup", "--pair", "K7QM4XPB2RHD", "--no-desktop", "--provide-spaces"]);
    expect(joinArgs("K7QM4XPB2RHD", "https://r.example")).toEqual(["host", "setup", "--pair", "K7QM4XPB2RHD", "--relay", "https://r.example", "--no-desktop", "--provide-spaces"]);
  });
  it("shows the last line cua wrote, else the exit status", () => {
    expect(failureText("warning: x\nerror: this pairing code was already used\n\n", "exited with 1")).toBe("error: this pairing code was already used");
    expect(failureText("", "exited with 1")).toBe("cua host setup exited with 1");
  });
});

describe.skipIf(process.platform === "win32")("running cua", () => {
  const dir = mkdtempSync(path.join(tmpdir(), "join-cua-"));
  afterAll(() => rmSync(dir, { recursive: true, force: true }));
  const script = (name: string, body: string) => {
    const file = path.join(dir, name);
    writeFileSync(file, `#!/bin/sh\n${body}\n`);
    chmodSync(file, 0o755);
    return file;
  };
  it("succeeds on exit 0 and receives the arguments", async () => {
    const out = path.join(dir, "args.txt");
    const cua = script("ok", `printf '%s\\n' "$@" > "${out}"`);
    expect((await runCua(cua, joinArgs("K7QM4XPB2RHD", null), {})).ok).toBe(true);
    expect((await import("node:fs")).readFileSync(out, "utf8").trim().split("\n")).toEqual(["host", "setup", "--pair", "K7QM4XPB2RHD", "--no-desktop", "--provide-spaces"]);
  });
  it("fails with what cua said on stderr", async () => {
    const cua = script("bad", `echo "host setup failed at register: the pairing code expired" >&2; exit 1`);
    const r = await runCua(cua, [], {});
    expect(r.ok).toBe(false);
    expect(failureText(r.stderr, r.status)).toBe("host setup failed at register: the pairing code expired");
  });
  it("stops a run that takes too long", async () => {
    const cua = script("slow", "sleep 5");
    expect(await runCua(cua, [], {}, 100)).toMatchObject({ ok: false, status: "timed out" });
  });
  it("reports a cua that cannot start", async () => {
    expect((await runCua(path.join(dir, "missing"), [], {})).ok).toBe(false);
  });
});

describe("host.joinWithCode", () => {
  const status = { configured: true };
  const make = (run: (cua: string, args: string[]) => Promise<CuaRun>, supervisor: unknown = { cua: "/app/cua" }) => {
    const calls: { cua: string; args: string[] }[] = [];
    let refreshed = 0;
    const ctx = {
      model: { host: { refresh: async () => void refreshed++, state: status, machineId: "m1", progress: "" } },
      supervisor,
      env: { HOME: "/h" },
    } as unknown as BridgeContext;
    const method = joinMethods(ctx, async (cua, args) => (calls.push({ cua, args }), run(cua, args)))["host.joinWithCode"]!;
    const handler = (args: Record<string, unknown>) => Promise.resolve(method(args as never, null as never));
    return { handler, calls, refreshed: () => refreshed };
  };
  const code = async (p: Promise<unknown>) => ((await p.then(() => null, (e: unknown) => e)) as Failure | null)?.code;

  it("runs the bundled cua with the canonical code, refreshes the host and answers its status", async () => {
    const t = make(async () => ({ ok: true, stderr: "", status: "exited with 0" }));
    const answer = (await t.handler({ code: "k7qm-4xpb-2rhd" })) as Record<string, unknown>;
    expect(t.calls).toEqual([{ cua: "/app/cua", args: ["host", "setup", "--pair", "K7QM4XPB2RHD", "--no-desktop", "--provide-spaces"] }]);
    expect(t.refreshed()).toBe(1);
    expect(answer).toMatchObject({ configured: true, machineId: "m1" });
  });
  it("passes a relay from the page, or from a pasted link", async () => {
    const t = make(async () => ({ ok: true, stderr: "", status: "" }));
    await t.handler({ code: "K7QM4XPB2RHD", relay: "https://r.example" });
    await t.handler({ code: "https://cua.ai/add/K7QM-4XPB-2RHD?relay=https%3A%2F%2Fs.example" });
    expect(t.calls.map((c) => c.args.slice(4))).toEqual([["--relay", "https://r.example", "--no-desktop", "--provide-spaces"], ["--relay", "https://s.example", "--no-desktop", "--provide-spaces"]]);
  });
  it("rejects a bad code or relay without running anything", async () => {
    const t = make(async () => ({ ok: true, stderr: "", status: "" }));
    expect(await code(t.handler({ code: "nope" }))).toBe("bad_args");
    expect(await code(t.handler({}))).toBe("bad_args");
    expect(await code(t.handler({ code: "K7QM4XPB2RHD", relay: "ftp://x" }))).toBe("bad_args");
    expect(t.calls).toEqual([]);
  });
  it("fails with cua's own words, and does not refresh", async () => {
    const t = make(async () => ({ ok: false, stderr: "this pairing code was already used\n", status: "exited with 1" }));
    await expect(t.handler({ code: "K7QM4XPB2RHD" })).rejects.toMatchObject({ code: "failed", message: "this pairing code was already used" });
    expect(t.refreshed()).toBe(0);
  });
  it("is unsupported in a build without the bundled cua", async () => {
    const t = make(async () => ({ ok: true, stderr: "", status: "" }), null);
    expect(await code(t.handler({ code: "K7QM4XPB2RHD" }))).toBe("unsupported");
  });
});

describe("cua:// links", () => {
  it("opens Machines with the code", () => {
    expect(routeForDeepLink("cua://add/K7QM-4XPB-2RHD")).toBe("/machines?join=K7QM4XPB2RHD");
    expect(routeForDeepLink("cua://add/k7qm4xpb2rhd")).toBe("/machines?join=K7QM4XPB2RHD");
    expect(routeForDeepLink("cua://add?code=K7QM-4XPB-2RHD")).toBe("/machines?join=K7QM4XPB2RHD");
  });
  it("carries an http(s) relay", () => {
    expect(routeForDeepLink("cua://add/K7QM-4XPB-2RHD?relay=https%3A%2F%2Fr.example")).toBe("/machines?join=K7QM4XPB2RHD&relay=https%3A%2F%2Fr.example");
    expect(routeForDeepLink("cua://add/K7QM-4XPB-2RHD?relay=javascript%3Aalert(1)")).toBe("/machines?join=K7QM4XPB2RHD");
  });
  it("ignores anything else", () => {
    for (const bad of ["cua://keyvault", "cua://add", "cua://add/short", "cua://add/K7QM-4XPB-2RH!", "https://cua.ai/add/K7QM-4XPB-2RHD", "cua-spaces://app/machines", "not a url"]) {
      expect(routeForDeepLink(bad)).toBeNull();
    }
  });
  it("finds the link among a command line's arguments", () => {
    expect(deepLinkInArgv(["/app/Cua Spaces.exe", "--flag", "cua://add/K7QM-4XPB-2RHD"])).toBe("cua://add/K7QM-4XPB-2RHD");
    expect(deepLinkInArgv(["/app/Cua Spaces.exe"])).toBeNull();
  });
});
