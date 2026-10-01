// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";

import {
  cloudNamespaceOf,
  displayName,
  hasFeature,
  nameOf,
  providerOfId,
  rowToSpace,
  type SpaceRow,
} from "./spaces";

function row(overrides: Partial<SpaceRow> = {}): SpaceRow {
  return {
    id: "space://cloud/cua-spaces-abc/brave-otter",
    name: "brave-otter",
    provider: "cloud",
    spacesdVersion: "0.4.0",
    features: ["desktop_stream", "window_stream"],
    addedAt: "2026-08-30T12:00:00Z",
    os: "linux",
    reachable: true,
    ...overrides,
  };
}

describe("rowToSpace", () => {
  it("maps a reachable cloud row onto a running tile grouped by namespace", () => {
    const space = rowToSpace(row(), 1_000);
    expect(space.id).toBe("space://cloud/cua-spaces-abc/brave-otter");
    expect(space.name).toBe("Brave Otter");
    expect(space.status).toBe("running");
    expect(space.provider).toBe("cloud");
    expect(space.fleetId).toBe("cua-spaces-abc");
    expect(space.detail).toBe("cua-spaces-abc");
    expect(space.lastUsedAt).toBe(Date.parse("2026-08-30T12:00:00Z"));
    expect(space.sdk?.features).toContain("desktop_stream");
  });

  it("shows an unreachable Space as suspended with the reason", () => {
    const space = rowToSpace(row({ reachable: false, error: "timed out" }), 5);
    expect(space.status).toBe("suspended");
    expect(space.detail).toContain("timed out");
  });

  it("maps direct and local rows, defaulting the OS to linux", () => {
    const direct = rowToSpace(
      row({ id: "space://direct/10.0.0.5:3211", name: "10.0.0.5:3211", provider: "direct", os: undefined, addedAt: undefined }),
      42,
    );
    expect(direct.os).toBe("linux");
    expect(direct.detail).toBe("10.0.0.5:3211");
    expect(direct.name).toBe("10.0.0.5:3211");
    expect(direct.lastUsedAt).toBe(42);
    expect(direct.fleetId).toBe("direct");
    const mac = rowToSpace(row({ id: "space://local/space-1", provider: "local", os: "macos" }), 0);
    expect(mac.scene).toBe("mac-desktop");
    expect(mac.detail).toBe("This Mac");
  });
});

describe("ids and features", () => {
  it("reads locations and namespaces from space ids", () => {
    expect(providerOfId("space://direct/h:1")).toBe("direct");
    expect(providerOfId("space://local/x")).toBe("local");
    expect(providerOfId("pending:1")).toBeUndefined();
    expect(cloudNamespaceOf("space://cloud/ns/box")).toBe("ns");
    expect(cloudNamespaceOf("space://local/x")).toBeUndefined();
  });

  it("reads the unified refs the SDK prints", () => {
    expect(providerOfId("cloud:box")).toBe("cloud");
    expect(providerOfId("local:box")).toBe("local");
    expect(providerOfId("direct:10.0.0.5:3211")).toBe("direct");
    expect(providerOfId("relay:0123abcd4567")).toBe("relay");
    expect(cloudNamespaceOf("cloud:box")).toBeUndefined();
    const cloud = rowToSpace(row({ id: "cloud:box", provider: "cloud" }), 0);
    expect(cloud.provider).toBe("cloud");
    expect(cloud.fleetId).toBe("cloud");
    expect(cloud.detail).toBe("Cua Cloud");
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

  it("title-cases Space names", () => {
    expect(displayName("brave-otter")).toBe("Brave Otter");
    expect(displayName("space-3f2a")).toBe("Space 3f2a");
  });
});

describe("nameOf", () => {
  it("prefers the id's name over a container hostname", () => {
    expect(nameOf({ id: "local:cua-e2e-desk", name: "69af41d9344c" })).toBe("cua-e2e-desk");
    expect(nameOf({ id: "local:desk", name: "My desk" })).toBe("My desk");
    expect(nameOf({ id: "direct:10.0.0.5:3211", name: "" })).toBe("direct:10.0.0.5:3211");
  });
});
