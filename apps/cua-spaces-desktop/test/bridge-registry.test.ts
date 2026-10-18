// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The bridge's method table and envelope: the same methods as the SwiftUI
// host (`WebUIBridge.methods`, in its order) and the web contract
// (`WEBKIT_METHODS`, `ELECTRON_HOST_METHODS`), each registered once, and
// requests answered in the native hosts' envelope with their error codes.
import { readFileSync } from "node:fs";
import * as path from "node:path";
import { describe, expect, it } from "vitest";
import { ELECTRON_HOST_METHODS, WEBKIT_METHODS } from "../../cua-spaces-web/src/bridge/webkit-protocol";
import { AREAS, orderedMethods } from "../src/bridge";
import type { BridgeContext } from "../src/bridge/context";
import { BridgeEvents, BridgeRegistry, Failure, unimplemented } from "../src/bridge/host";
import { ELECTRON_METHODS, METHODS } from "../src/bridge/methods";
import { NOT_YET } from "./not-yet";

/** `static let methods = [...]` in WebUIBridge.swift. */
function swiftMethods(): string[] {
  const file = path.resolve(__dirname, "../../cua-spaces-macos/Sources/CuaSpacesMacKit/WebHost/WebUIBridge.swift");
  const list = /static let methods = \[([\s\S]*?)\]/.exec(readFileSync(file, "utf8"))?.[1] ?? "";
  return [...list.matchAll(/"([^"]+)"/g)].map((m) => m[1]!);
}

/** A context for registering the areas (no handler runs). */
const stubContext = () =>
  ({
    model: { native: {}, startup: {} },
    events: new BridgeEvents(),
    ui: {},
    supervisor: null,
    version: "0.0.0",
    platform: "darwin",
    methods: () => [],
    env: {},
  }) as unknown as BridgeContext;

function registryOf(ctx = stubContext()): BridgeRegistry {
  const r = new BridgeRegistry();
  for (const area of AREAS) r.register(area(ctx));
  return r;
}

describe("the method table", () => {
  it("lists the SwiftUI host's methods in its order", () => {
    expect([...METHODS]).toEqual(swiftMethods());
  });

  it("matches the web contract: the webkit methods, plus Electron's own", () => {
    expect([...METHODS].sort()).toEqual([...WEBKIT_METHODS].sort());
    expect([...ELECTRON_METHODS]).toEqual([...ELECTRON_HOST_METHODS]);
  });

  it("registers every method once, one area each, in app.info's order", () => {
    const r = registryOf();
    expect(orderedMethods(r)).toEqual([...METHODS, ...ELECTRON_METHODS]);
    expect(() => r.register({ "app.info": () => null })).toThrow(/registered twice/);
  });

  it("answers unimplemented only for the areas not ported yet", async () => {
    const r = registryOf();
    const unanswered: string[] = [];
    for (const m of r.methods) {
      const reply = await r.dispatch({ id: m, method: m, args: {} });
      if (!reply.ok && reply.error.code === "unimplemented") unanswered.push(m);
    }
    expect(unanswered.sort()).toEqual([...NOT_YET].sort());
  });
});

describe("the envelope", () => {
  const r = new BridgeRegistry();
  r.register({
    "a.ok": (args) => ({ echo: args.x ?? null }),
    "a.void": () => undefined,
    "a.async": async () => [1, 2],
    "a.bad": () => {
      throw Failure.badArgs("x: string");
    },
    "a.presented": () => {
      throw new Failure("failed", "Couldn't set up", { title: "Setup failed", details: "relay said no", actionLabel: "Retry" });
    },
    "a.sdk": () => {
      const e = new Error("CuaError.NotFound: no Space local:x");
      Object.assign(e, { [Symbol.for("typeName")]: "CuaError", tag: "NotFound" });
      throw e;
    },
    "a.plain": () => {
      throw new Error("disk full");
    },
    ...unimplemented("a.later"),
  });

  it("echoes the id and answers the result, null for nothing", async () => {
    expect(await r.dispatch({ id: "1", method: "a.ok", args: { x: 2 } })).toEqual({ id: "1", ok: true, result: { echo: 2 } });
    expect(await r.dispatch({ id: "2", method: "a.void" })).toEqual({ id: "2", ok: true, result: null });
    expect(await r.dispatch({ id: "3", method: "a.async" })).toEqual({ id: "3", ok: true, result: [1, 2] });
  });

  it("answers a failure's code, and the SDK's words for its errors", async () => {
    expect(await r.dispatch({ id: "1", method: "a.bad" })).toEqual({ id: "1", ok: false, error: { code: "bad_args", message: "x: string" } });
    expect(await r.dispatch({ id: "2", method: "a.sdk" })).toEqual({ id: "2", ok: false, error: { code: "failed", message: "no Space local:x" } });
    expect(await r.dispatch({ id: "3", method: "a.plain" })).toEqual({ id: "3", ok: false, error: { code: "failed", message: "disk full" } });
    expect(await r.dispatch({ id: "4", method: "a.presented" })).toEqual({
      id: "4",
      ok: false,
      error: { code: "failed", message: "Couldn't set up", title: "Setup failed", details: "relay said no", actionLabel: "Retry" },
    });
  });

  it("refuses a request without a method, and a method it does not route", async () => {
    expect(await r.dispatch({ id: "1" })).toMatchObject({ id: "1", ok: false, error: { code: "bad_args" } });
    expect(await r.dispatch(null)).toMatchObject({ id: "", ok: false, error: { code: "bad_args" } });
    expect(await r.dispatch({ id: "2", method: "spaces.nope" })).toMatchObject({ ok: false, error: { code: "unimplemented" } });
    expect(await r.dispatch({ id: "3", method: "a.later" })).toMatchObject({ ok: false, error: { code: "unimplemented" } });
  });

  it("passes the calling window to the handler", async () => {
    const seen: unknown[] = [];
    const w = new BridgeRegistry();
    w.register({ "w.x": (_a, caller) => void seen.push(caller.window) });
    await w.dispatch({ id: "1", method: "w.x" }, { window: "win-1" });
    expect(seen).toEqual(["win-1"]);
  });
});

describe("events", () => {
  it("go to every listener as { event, payload }", () => {
    const events = new BridgeEvents();
    const got: unknown[] = [];
    const off = events.subscribe((e) => got.push(e));
    events.emit("spaces.changed");
    events.emit("startup.changed", { phase: "ready" });
    off();
    events.emit("session.changed");
    expect(got).toEqual([{ event: "spaces.changed" }, { event: "startup.changed", payload: { phase: "ready" } }]);
  });
});
