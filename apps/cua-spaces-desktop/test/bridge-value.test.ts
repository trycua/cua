// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The core's values as the page reads them: the same JSON as the SwiftUI
// host's `BridgeValue.encode` (records with camelCase fields and nulls,
// enums as their Swift case names, `{type, ...fields}` with a payload).
import { describe, expect, it } from "vitest";
import { caseName, encode } from "../src/bridge/value";

const TYPE = Symbol.for("typeName");

/** A UniFFI enum case as the generated bindings make it. */
function tagged(typeName: string, tag: string, inner?: unknown) {
  const v: Record<string | symbol, unknown> = { [TYPE]: typeName, tag };
  if (inner !== undefined) v.inner = inner;
  return v;
}

describe("caseName", () => {
  it("spells a Rust variant as Swift's case", () => {
    expect(caseName("TouchId")).toBe("touchId");
    expect(caseName("Macos")).toBe("macos");
    expect(caseName("AwsEcr")).toBe("awsEcr");
    expect(caseName("NotApplicable")).toBe("notApplicable");
    expect(caseName("HTTPError")).toBe("httpError");
    expect(caseName("Ok")).toBe("ok");
  });
});

describe("encode", () => {
  it("keeps strings, booleans and finite numbers; 64-bit integers are numbers", () => {
    expect(encode("a")).toBe("a");
    expect(encode(true)).toBe(true);
    expect(encode(1.5)).toBe(1.5);
    expect(encode(Number.NaN)).toBeNull();
    expect(encode(BigInt("1700000000000"))).toBe(1_700_000_000_000);
  });

  it("writes a record's missing optionals as null, as Swift's nil", () => {
    expect(encode({ id: "s1", startedAt: undefined, sdk: null, lastUsedAt: BigInt(5) })).toEqual({ id: "s1", startedAt: null, sdk: null, lastUsedAt: 5 });
  });

  it("gives an enum case without a payload as its name, with one as {type, ...fields}", () => {
    expect(encode(tagged("AppSignInPhase", "Idle"))).toBe("idle");
    expect(encode(tagged("AppSignInPhase", "Waiting", { userCode: "ABCD-1234" }))).toEqual({ type: "waiting", userCode: "ABCD-1234" });
    expect(encode(tagged("AppSignInPhase", "Waiting", { userCode: undefined }))).toEqual({ type: "waiting", userCode: null });
    expect(encode(tagged("KvSelection", "Category", ["all"]))).toEqual({ type: "category", value: "all" });
    expect(encode(tagged("X", "Pair", ["a", BigInt(2)]))).toEqual({ type: "pair", value: "a", value1: 2 });
  });

  it("writes bytes as base64, dates as milliseconds, maps as objects and lists as arrays", () => {
    expect(encode(new Uint8Array([0x89, 0x50, 0x4e, 0x47]))).toBe("iVBORw==");
    expect(encode(new Date(1_700_000_000_000))).toBe(1_700_000_000_000);
    expect(encode(new Map([["docker", "not running"]]))).toEqual({ docker: "not running" });
    expect(encode([tagged("AppSpaceOs", "Linux"), "macos"])).toEqual(["linux", "macos"]);
  });

  it("leaves live handles out as null", () => {
    class Handle {
      uniffiDestroy() {}
    }
    expect(encode(new Handle())).toBeNull();
    expect(encode({ [Symbol.for("pointer")]: 1n, x: 1 })).toBeNull();
    expect(encode({ cua: new Handle(), name: "a" })).toEqual({ cua: null, name: "a" });
  });
});
