// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import {
  displayName,
  hasFeature,
  nameOf,
  providerOfId,
  rowToSpace,
  type SpaceRow,
} from "./spaces";

function row(overrides: Partial<SpaceRow> = {}): SpaceRow {
  return {
    id: "space://direct/10.0.0.5:3211",
    name: "brave-otter",
    provider: "direct",
    spacesdVersion: "0.4.0",
    features: ["desktop_stream", "window_stream"],
    addedAt: "2026-08-30T12:00:00Z",
    os: "linux",
    reachable: true,
    ...overrides,
  };
}

describe("rowToSpace", () => {
  it("maps a reachable row onto a running tile", () => {
    const space = rowToSpace(row(), 1_000);
    expect(space.id).toBe("space://direct/10.0.0.5:3211");
    expect(space.name).toBe("brave-otter");
    expect(space.status).toBe("running");
    expect(space.provider).toBe("direct");
    expect(space.detail).toBe("10.0.0.5:3211");
    expect(space.lastUsedAt).toBe(Date.parse("2026-08-30T12:00:00Z"));
    expect(space.sdk?.features).toContain("desktop_stream");
  });

  it("shows an unreachable Space as suspended with the reason", () => {
    const space = rowToSpace(row({ reachable: false, error: "timed out" }), 5);
    expect(space.status).toBe("suspended");
    expect(space.detail).toContain("timed out");
  });

  it("maps direct and local rows, preserving an unknown OS", () => {
    const direct = rowToSpace(
      row({ id: "space://direct/10.0.0.5:3211", name: "10.0.0.5:3211", provider: "direct", os: undefined, addedAt: undefined }),
      42,
    );
    expect(direct.os).toBe("unknown");
    expect(direct.detail).toBe("10.0.0.5:3211");
    expect(direct.name).toBe("10.0.0.5:3211");
    expect(direct.lastUsedAt).toBe(42);
    const mac = rowToSpace(row({ id: "space://local/space-1", provider: "local", os: "macos" }), 0);
    expect(mac.scene).toBe("mac-desktop");
    expect(mac.detail).toBe("This Mac");
  });
});

describe("ids and features", () => {
  it("reads locations from space ids", () => {
    expect(providerOfId("space://direct/h:1")).toBe("direct");
    expect(providerOfId("space://local/x")).toBe("local");
    expect(providerOfId("pending:1")).toBeUndefined();
  });

  it("reads the unified refs the SDK prints", () => {
    expect(providerOfId("local:box")).toBe("local");
    expect(providerOfId("direct:10.0.0.5:3211")).toBe("direct");
    expect(providerOfId("relay:0123abcd4567")).toBe("relay");
    const direct = rowToSpace(
      row({ id: "direct:10.0.0.5:3211", name: "10.0.0.5:3211", provider: "direct" }),
      0,
    );
    expect(direct.detail).toBe("10.0.0.5:3211");
  });

  it("checks features from the SDK ref only", () => {
    expect(hasFeature(rowToSpace(row(), 0), "desktop_stream")).toBe(true);
    expect(hasFeature({ sdk: undefined }, "desktop_stream")).toBe(false);
  });

  it("shows Space names as the user typed them, trimmed", () => {
    expect(displayName("brave-otter")).toBe("brave-otter");
    expect(displayName(" space-3f2a ")).toBe("space-3f2a");
  });
});

describe("nameOf", () => {
  it("prefers the id's name over a container hostname", () => {
    expect(nameOf({ id: "local:cua-e2e-desk", name: "69af41d9344c" })).toBe("cua-e2e-desk");
    expect(nameOf({ id: "local:desk", name: "My desk" })).toBe("My desk");
    expect(nameOf({ id: "direct:10.0.0.5:3211", name: "" })).toBe("direct:10.0.0.5:3211");
  });
});
