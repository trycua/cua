// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The GitHub sign-in methods (`github.*`): each calls its own daemon method with
// only its own arguments, and a bad call fails before the daemon is asked.
import { describe, expect, it } from "vitest";
import type { BridgeContext } from "../src/bridge/context";
import { githubMethods } from "../src/bridge/github";
import type { Handlers } from "../src/bridge/host";

const calls: [string, Record<string, unknown> | undefined][] = [];
const handlers = (): Handlers => {
  calls.length = 0;
  return githubMethods({} as BridgeContext, () => async (name, args) => (calls.push([name, args]), { hosts: [] }));
};
const run = (h: Handlers, m: string, args: Record<string, unknown> = {}) => (h[m] as (a: Record<string, unknown>) => Promise<unknown>)(args);

describe("github.*", () => {
  it("calls the daemon method of the same name with only its own arguments", async () => {
    const h = handlers();
    await run(h, "github.status", { token: "must-not-travel" });
    await run(h, "github.rescan");
    await run(h, "github.use", { host: "github.com", login: "bob", token: "must-not-travel" });
    await run(h, "github.use", { host: "github.com" });
    await run(h, "github.use", { host: "github.com", login: null });
    await run(h, "github.enable", { host: "ghe.example.com", enabled: false, extra: 1 });
    expect(calls).toEqual([
      ["github.status", {}],
      ["github.rescan", {}],
      ["github.use", { host: "github.com", login: "bob" }],
      // No login: gh's own account.
      ["github.use", { host: "github.com" }],
      ["github.use", { host: "github.com" }],
      ["github.enable", { host: "ghe.example.com", enabled: false }],
    ]);
  });

  it("fails before the daemon is asked when a host or a switch is missing", async () => {
    const h = handlers();
    await expect(run(h, "github.use", { login: "bob" })).rejects.toMatchObject({ code: "bad_args" });
    await expect(run(h, "github.enable", { host: "github.com" })).rejects.toMatchObject({ code: "bad_args" });
    await expect(run(h, "github.enable", { host: "github.com", enabled: "no" })).rejects.toMatchObject({ code: "bad_args" });
    expect(calls).toEqual([]);
  });

  it("is unsupported without the cua daemon", async () => {
    const h = githubMethods({ model: { agents: {}, native: {} } } as unknown as BridgeContext, () => {
      throw new Error("The GitHub sign-in needs the cua daemon");
    });
    await expect(run(h, "github.status")).rejects.toThrow(/needs the cua daemon/);
  });
});
