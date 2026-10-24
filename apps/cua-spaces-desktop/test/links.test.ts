// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `links.*`: the SDK's shares through the backend, answering the web
// contract's shapes.
import { describe, expect, it, vi } from "vitest";
import { OP_SHAPES, SHAPE_DEFS } from "../../cua-spaces-web/src/bridge/contracts/shapes";
import { validate } from "../../cua-spaces-web/src/bridge/contracts/schema";
import type { BridgeContext } from "../src/bridge/context";
import { linksMethods } from "../src/bridge/links";
import type { ShareAccess } from "../src/native/generated/index";
import { LiveSpacesBackend } from "../src/model/backend";

const TEAM = "team" as ShareAccess;
const PUBLIC = "public" as ShareAccess;
const SHARE = { id: "share-0123456789ab", url: "https://s1.m.relay.example/", name: undefined, access: TEAM, kind: "port", target: "port 3000", owner: "ada@example.com", createdAtUnix: 1000n, expiresAtUnix: undefined };

function backend(cua: Record<string, unknown>) {
  return new LiveSpacesBackend({} as never, cua as never);
}

describe("links.*", () => {
  it("lists the SDK's shares in the page's shape", async () => {
    const b = backend({ shares: vi.fn(async () => [SHARE]) });
    const ctx = { model: { backend: b } } as unknown as BridgeContext;
    const r = await linksMethods(ctx)["links.list"]!({}, {});
    expect(r).toEqual({ links: [{ ...SHARE, name: null, access: "team", createdAtUnix: 1000, expiresAtUnix: null }] });
    expect(validate(OP_SHAPES["links.list"], r, SHAPE_DEFS)).toEqual([]);
  });

  it("creates with the access and lifetime asked for, and stops one or all", async () => {
    const share = vi.fn(async () => ({ ...SHARE, access: PUBLIC }));
    const stopShare = vi.fn(async (id: string) => [id]);
    const stopAllShares = vi.fn(async () => ["a", "b"]);
    const ctx = { model: { backend: backend({ share, stopShare, stopAllShares }) } } as unknown as BridgeContext;
    const m = linksMethods(ctx);
    const r = await m["links.create"]!({ target: "space:dev:3000", access: "public", ttlSeconds: 600 }, {});
    expect(share).toHaveBeenCalledWith("space:dev:3000", PUBLIC, 600, undefined);
    expect(validate(OP_SHAPES["links.create"], r, SHAPE_DEFS)).toEqual([]);
    expect(await m["links.stop"]!({ id: "share-1" }, {})).toEqual({ stopped: ["share-1"] });
    expect(await m["links.stop"]!({ id: "all" }, {})).toEqual({ stopped: ["a", "b"] });
    expect(stopAllShares).toHaveBeenCalledTimes(1);
  });

  it("a machine that is not on the relay says so", async () => {
    const share = vi.fn(async () => {
      throw new Error("this machine is not on your cua.ai relay, so a link would only open here. Run `cua host setup` once, then share again");
    });
    const ctx = { model: { backend: backend({ share }) } } as unknown as BridgeContext;
    await expect(linksMethods(ctx)["links.create"]!({ target: "3000" }, {})).rejects.toThrow(/cua host setup/);
  });

  it("needs a target", async () => {
    const ctx = { model: { backend: backend({}) } } as unknown as BridgeContext;
    await expect(linksMethods(ctx)["links.create"]!({}, {})).rejects.toThrow(/target/);
  });
});
