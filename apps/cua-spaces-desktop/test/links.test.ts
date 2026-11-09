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
const SHARE = {
  id: "share-0123456789ab",
  url: "https://s1.m.relay.example/",
  name: undefined,
  access: TEAM,
  kind: "port",
  target: "port 3000",
  owner: "ada@example.com",
  createdAtUnix: 1000n,
  expiresAtUnix: undefined,
};

function backend(cua: Record<string, unknown>) {
  return new LiveSpacesBackend({} as never, cua as never);
}

describe("links.*", () => {
  it("lists the SDK's shares in the page's shape", async () => {
    const b = backend({ shares: vi.fn(async () => [SHARE]) });
    const ctx = { model: { backend: b } } as unknown as BridgeContext;
    const r = await linksMethods(ctx)["links.list"]!({}, {});
    expect(r).toEqual({
      links: [
        {
          ...SHARE,
          name: null,
          access: "team",
          createdAtUnix: 1000,
          expiresAtUnix: null,
          tcpConnectWith: null,
          tcpAddress: null,
          route: null,
          routeLine: null,
          allow: [],
          waiting: null,
          waitingText: null,
        },
      ],
    });
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

  it("a TCP share carries its route, who may connect and what it waits for", async () => {
    const shareTcp = vi.fn(async () => ({
      ...SHARE,
      access: PUBLIC,
      kind: "space_port",
      target: "space:mc:25565",
      tcpConnectWith: "203.0.113.24:26500",
      tcpAddress: "203.0.113.24:26500",
      route: "host",
      routeLine: "Through your AWS account",
      allow: ["203.0.113.0/24"],
      waiting: undefined,
      waitingText: undefined,
    }));
    const ctx = { model: { backend: backend({ shareTcp }) } } as unknown as BridgeContext;
    const r = await linksMethods(ctx)["links.create"]!(
      { target: "space:mc:25565", tcp: true, service: "minecraft", route: "host", allow: ["203.0.113.0/24"], checkHost: true, consent: true },
      {},
    );
    expect(shareTcp).toHaveBeenCalledWith("space:mc:25565", "minecraft", undefined, undefined, "host", ["203.0.113.0/24"], true, true);
    expect(r).toMatchObject({ route: "host", routeLine: "Through your AWS account", allow: ["203.0.113.0/24"], waiting: null, waitingText: null });
    expect(validate(OP_SHAPES["links.create"], r, SHAPE_DEFS)).toEqual([]);
  });

  it("approves and turns down a waiting share, and removes the share edge", async () => {
    const live = { ...SHARE, access: PUBLIC, tcpConnectWith: "x:1", route: "edge", routeLine: "Through your AWS account" };
    const approveShare = vi.fn(async () => live);
    const denyShare = vi.fn(async () => undefined);
    const removeShareEdge = vi.fn(async () => ["share-1"]);
    const m = linksMethods({ model: { backend: backend({ approveShare, denyShare, removeShareEdge }) } } as unknown as BridgeContext);
    const a = await m["links.approve"]!({ id: "share-1", always: true }, {});
    expect(approveShare).toHaveBeenCalledWith("share-1", true);
    expect(validate(OP_SHAPES["links.approve"], a, SHAPE_DEFS)).toEqual([]);
    expect(await m["links.deny"]!({ id: "share-1" }, {})).toEqual({ denied: "share-1" });
    expect(denyShare).toHaveBeenCalledWith("share-1");
    expect(await m["shares.edge.remove"]!({}, {})).toEqual({ ended: ["share-1"] });
  });

  it("reads the share settings in the page's words", async () => {
    const shareSettings = vi.fn(async () => ({
      landing: "OnlyMyCloud",
      relayFallback: false,
      dailyCapMib: 2048n,
      defaultAllow: [],
      approveAgentShares: true,
      alwaysAllowSpaces: ["mc"],
      edgeConsent: ["AWS"],
      usedToday: 1_503_238_553n,
      edgeCloud: "AWS",
      edgeHost: "ec2-198-51-100-7.us-west-2.compute.amazonaws.com",
      edgeMonthlyUsd: 7,
    }));
    const m = linksMethods({ model: { backend: backend({ shareSettings }) } } as unknown as BridgeContext);
    const r = await m["shares.settings.get"]!({}, {});
    expect(r).toMatchObject({ landing: "only_my_cloud", dailyCapMib: 2048, usedToday: 1_503_238_553, edgeCloud: "AWS", edgeMonthlyUsd: 7 });
    expect(validate(OP_SHAPES["shares.settings.get"], r, SHAPE_DEFS)).toEqual([]);
  });
});
