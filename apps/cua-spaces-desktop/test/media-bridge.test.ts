// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from "vitest";
import {
  createStreamOpener,
  decodeOpenMediaBridgeResponse,
  encodeOpenMediaBridgeRequest,
  grpcWebFrame,
  openMediaRequest,
  parseGrpcWebResponse,
  readDaemonEndpoint,
  type DaemonEndpoint,
  type Fetch,
} from "../src/media-bridge";

// Hand-encoded protobuf, checked against what protoc would write.
const bytes = (...xs: (number | string)[]) => Uint8Array.from(xs.flatMap((x) => (typeof x === "string" ? [...new TextEncoder().encode(x)] : [x])));
const field = (n: number, s: string) => [(n << 3) | 2, new TextEncoder().encode(s).length, s] as const;

function responseMessage(wsUrl: string, ticket: string, seconds: number | null, json = "{}"): Uint8Array {
  const ts = seconds === null ? [] : [0x1a, 0, 0x08, ...varint(seconds)];
  if (ts.length) ts[1] = ts.length - 2;
  return bytes(...field(1, wsUrl), ...field(2, ticket), ...ts, ...field(4, json));
}
function varint(n: number): number[] {
  const out: number[] = [];
  while (n > 0x7f) {
    out.push((n & 0x7f) | 0x80);
    n = Math.floor(n / 128);
  }
  out.push(n);
  return out;
}
function trailer(text: string): Uint8Array {
  const t = new TextEncoder().encode(text);
  const out = new Uint8Array(5 + t.length);
  out[0] = 0x80;
  new DataView(out.buffer).setUint32(1, t.length);
  out.set(t, 5);
  return out;
}
const concat = (...parts: Uint8Array[]) => Uint8Array.from(parts.flatMap((p) => [...p]));
const headers = (h: Record<string, string> = {}) => ({ get: (k: string) => h[k.toLowerCase()] ?? null });

const ENDPOINT: DaemonEndpoint = { loopbackUrl: "http://127.0.0.1:5123", token: "tok" };
const EXPIRES = 1_791_216_000; // 2026-10-05T…Z

function fakeDaemon(reply: (body: Uint8Array) => { status?: number; body: Uint8Array; headers?: Record<string, string> }) {
  const calls: { url: string; headers: Record<string, string>; body: Uint8Array }[] = [];
  const fetch: Fetch = async (url, init) => {
    calls.push({ url, headers: init.headers, body: init.body });
    const r = reply(init.body);
    const status = r.status ?? 200;
    return { ok: status < 300, status, headers: headers(r.headers), arrayBuffer: async () => r.body.slice().buffer };
  };
  return { fetch, calls };
}

describe("protobuf", () => {
  it("encodes OpenMediaBridgeRequest as fields 1 and 2", () => {
    expect([...encodeOpenMediaBridgeRequest("local:a", "{}")]).toEqual([...bytes(...field(1, "local:a"), ...field(2, "{}"))]);
    // Empty strings are left out, as proto3 does.
    expect([...encodeOpenMediaBridgeRequest("x", "")]).toEqual([...bytes(...field(1, "x"))]);
  });

  it("encodes long strings with a multi-byte length", () => {
    const json = "x".repeat(300);
    const enc = encodeOpenMediaBridgeRequest("n", json);
    expect([...enc.subarray(3, 6)]).toEqual([0x12, 0xac, 0x02]);
    expect(enc.length).toBe(3 + 3 + 300);
  });

  it("decodes OpenMediaBridgeResponse, Timestamp included, and skips unknown fields", () => {
    const msg = concat(responseMessage("ws://127.0.0.1:5123/v1/bridge/media?ticket=t1", "t1", EXPIRES, '{"codec":"MEDIA_CODEC_H264"}'), bytes(0x28, 0x05));
    expect(decodeOpenMediaBridgeResponse(msg)).toEqual({
      wsUrl: "ws://127.0.0.1:5123/v1/bridge/media?ticket=t1",
      ticket: "t1",
      expiresAt: EXPIRES,
      openMediaResponseJson: '{"codec":"MEDIA_CODEC_H264"}',
    });
    expect(decodeOpenMediaBridgeResponse(responseMessage("ws://h/m", "t", null)).expiresAt).toBeNull();
  });
});

describe("gRPC-Web", () => {
  it("frames a message with a zero flag and a big-endian length", () => {
    expect([...grpcWebFrame(bytes(1, 2, 3))]).toEqual([0, 0, 0, 0, 3, 1, 2, 3]);
  });

  it("reads the data frame and the trailer frame", () => {
    const body = concat(grpcWebFrame(bytes(9)), trailer("grpc-status:0\r\ngrpc-message:\r\n"));
    expect(parseGrpcWebResponse(body, headers())).toEqual({ message: bytes(9), status: 0, statusMessage: "" });
  });

  it("reads a trailers-only error from the headers, percent-decoded", () => {
    const r = parseGrpcWebResponse(new Uint8Array(), headers({ "grpc-status": "5", "grpc-message": "no%20such%20sandbox" }));
    expect(r).toEqual({ message: null, status: 5, statusMessage: "no such sandbox" });
  });
});

describe("readDaemonEndpoint", () => {
  const read = (doc: unknown) => async () => JSON.stringify(doc);

  it("reads the loopback URL and token from daemon.json", async () => {
    expect(await readDaemonEndpoint("/h", read({ pid: 1, loopback_url: "http://127.0.0.1:5123/", token: "tok", version: "0.2.0" }))).toEqual(ENDPOINT);
  });

  it("refuses a daemon.json without a loopback listener or token, or one pointing off this machine", async () => {
    expect(await readDaemonEndpoint("/h", read({ pid: 1, socket_path: "/h/cua.sock", loopback_url: null, token: null }))).toBeNull();
    expect(await readDaemonEndpoint("/h", read({ loopback_url: "http://127.0.0.1:5123", token: "" }))).toBeNull();
    expect(await readDaemonEndpoint("/h", read({ loopback_url: "http://10.0.0.5:5123", token: "tok" }))).toBeNull();
    expect(await readDaemonEndpoint("/h", async () => Promise.reject(new Error("ENOENT")))).toBeNull();
  });
});

describe("openMediaRequest", () => {
  it("asks for the tile tier the macOS app uses, view only", () => {
    expect(openMediaRequest("tile")).toEqual({
      target: { displayId: "primary" },
      maxFps: 10,
      maxDimension: 960,
      policy: "SESSION_POLICY_VIEW_ONLY",
      geometryControl: "GEOMETRY_CONTROL_OBSERVE_ONLY",
      ticketTtl: "120s",
    });
  });

  it("takes the Space's defaults and input for the viewer", () => {
    const full = openMediaRequest("full");
    expect(full).not.toHaveProperty("maxFps");
    expect(full).not.toHaveProperty("maxDimension");
    expect(full.policy).toBe("SESSION_POLICY_ALLOW_ACTIVATION");
  });

  it("streams one window in the background, or activating it when the Space asked", () => {
    expect(openMediaRequest("full", { windowId: "w-1", epoch: 3 })).toEqual({
      target: { window: { id: "w-1", epoch: "3" } },
      policy: "SESSION_POLICY_BACKGROUND_ONLY",
      geometryControl: "GEOMETRY_CONTROL_OBSERVE_ONLY",
      ticketTtl: "120s",
    });
    expect(openMediaRequest("full", { windowId: "w-1", epoch: 3, activate: true }).policy).toBe("SESSION_POLICY_ALLOW_ACTIVATION");
    expect(openMediaRequest("tile", { windowId: "w-1" })).toMatchObject({ target: { window: { id: "w-1", epoch: "1" } }, maxFps: 10, policy: "SESSION_POLICY_VIEW_ONLY" });
    expect(openMediaRequest("full", { activate: true }).target).toEqual({ displayId: "primary" });
  });
});

describe("spaces.openStream", () => {
  const ok = () =>
    fakeDaemon(() => ({
      body: concat(grpcWebFrame(responseMessage("ws://127.0.0.1:5123/v1/bridge/media?ticket=t1", "t1", EXPIRES)), trailer("grpc-status:0\r\n")),
      headers: { "content-type": "application/grpc-web+proto" },
    }));
  const base = (fetch: Fetch, endpoints: (DaemonEndpoint | null)[] = [ENDPOINT]) => {
    let starts = 0;
    const opener = createStreamOpener({
      cuaHome: "/h",
      fetch,
      hasCua: async () => true,
      startDaemon: async () => void (starts += 1),
      readEndpoint: async () => (endpoints.length > 1 ? endpoints.shift()! : endpoints[0]!),
      sleep: async () => {},
    });
    return { opener, starts: () => starts };
  };

  it("calls OpenMediaBridge with the bearer token and the Space id, and answers the URL and expiry", async () => {
    const d = ok();
    const { opener } = base(d.fetch);
    expect(await opener({ spaceId: "local:r3-electron", tier: "tile" })).toEqual({
      wsUrl: "ws://127.0.0.1:5123/v1/bridge/media?ticket=t1",
      expiresAt: new Date(EXPIRES * 1000).toISOString(),
    });
    const call = d.calls[0]!;
    expect(call.url).toBe("http://127.0.0.1:5123/cua.daemon.v1.DaemonService/OpenMediaBridge");
    expect(call.headers).toMatchObject({ authorization: "Bearer tok", "content-type": "application/grpc-web+proto", "x-grpc-web": "1" });
    const sent = new TextDecoder().decode(call.body.subarray(5));
    expect(sent).toContain("local:r3-electron");
    expect(sent).toContain('"maxDimension":960');
  });

  it("starts the daemon when there is no daemon.json, then waits for it", async () => {
    const d = ok();
    const { opener, starts } = base(d.fetch, [null, null, ENDPOINT]);
    await opener({ spaceId: "local:a", tier: "full" });
    expect(starts()).toBe(1);
  });

  it("says unsupported without cua, so the page keeps its fallback", async () => {
    const opener = createStreamOpener({ cuaHome: "/h", fetch: ok().fetch, hasCua: async () => false, startDaemon: async () => {} });
    await expect(opener({ spaceId: "local:a", tier: "tile" })).rejects.toMatchObject({ code: "unsupported" });
  });

  it("fails a daemon that does not start as an error to try again, not as no video here", async () => {
    const d = ok();
    const endpoints: (DaemonEndpoint | null)[] = Array.from({ length: 42 }, () => null);
    const { opener, starts } = base(d.fetch, [...endpoints, ENDPOINT]);
    const failed = await opener({ spaceId: "local:a", tier: "full" }).then(
      () => null,
      (e: Error & { code?: string }) => e,
    );
    expect(failed?.message).toBe("The cua daemon didn't start, so live video can't open.");
    expect(failed?.code).toBeUndefined();
    // Try again: the daemon is up now.
    expect((await opener({ spaceId: "local:a", tier: "full" })).wsUrl).toContain("ticket=t1");
    expect(starts()).toBe(2);
  });

  it("passes the daemon's error on", async () => {
    const d = fakeDaemon(() => ({ body: new Uint8Array(), headers: { "grpc-status": "5", "grpc-message": "no sandbox named local:gone" } }));
    const { opener } = base(d.fetch);
    await expect(opener({ spaceId: "local:gone", tier: "tile" })).rejects.toThrow("no sandbox named local:gone");
  });

  it("restarts a daemon whose daemon.json outlived it, once", async () => {
    let n = 0;
    const good = ok();
    const fetch: Fetch = async (url, init) => {
      if (n++ === 0) throw new TypeError("fetch failed: ECONNREFUSED");
      return good.fetch(url, init);
    };
    const { opener, starts } = base(fetch);
    await opener({ spaceId: "local:a", tier: "tile" });
    expect(starts()).toBe(1);
  });

  it("checks its arguments", async () => {
    const { opener } = base(ok().fetch);
    await expect(opener({ spaceId: "", tier: "tile" })).rejects.toThrow("spaceId");
    await expect(opener({ spaceId: "a", tier: "big" as never })).rejects.toThrow("tier");
  });

  it("answers the test stream URL instead of the daemon when one is set", async () => {
    const opener = createStreamOpener({
      cuaHome: "/h",
      fetch: async () => {
        throw new Error("no daemon in this test");
      },
      hasCua: async () => false,
      startDaemon: async () => {},
      testStreamUrl: "ws://127.0.0.1:8787/media?ticket={id}&src={tier}",
    });
    expect(await opener({ spaceId: "local:a b", tier: "tile" })).toEqual({ wsUrl: "ws://127.0.0.1:8787/media?ticket=local%3Aa%20b&src=tile", expiresAt: null });
  });
});
