// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// What `spaces.create` forwards for an agent home (CUA-2630): the spawn capability and its `cua.home.*` labels.
import { describe, expect, it } from "vitest";
import { createExtrasOf } from "../src/model/backend";

describe("a create's extras", () => {
  it("carries the warming home's labels (hidden, off the relay) as they come", () => {
    expect(createExtrasOf({ image: "x", labels: { "cua.hidden": "home", "cua.relay": "off" } })).toEqual({
      labels: { "cua.hidden": "home", "cua.relay": "off" },
    });
  });

  it("carry an agent home's children and labels", () => {
    expect(createExtrasOf({ image: "x", children: true, labels: { "cua.home": "1", "cua.home.agent": "claude-code" } })).toEqual({
      children: true,
      labels: { "cua.home": "1", "cua.home.agent": "claude-code" },
    });
  });
  it("are empty for a plain create", () => {
    expect(createExtrasOf({ image: "x", network: "none" })).toEqual({});
  });
  it("ignore the wrong types and labels that are not text", () => {
    expect(createExtrasOf({ children: 1, labels: { a: 1, "": "x", b: "", c: "ok" } })).toEqual({ labels: { c: "ok" } });
    expect(createExtrasOf({ labels: ["a"] })).toEqual({});
  });
});
