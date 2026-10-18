// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// `spaces.openStream`: a media ticket for a Space's live desktop, minted by
// the cua daemon (`DaemonService.OpenMediaBridge`,
// libs/cua/proto/cua/daemon/v1/daemon.proto). The daemon opens the media
// session in the Space and hands back a loopback WebSocket URL with a bridge
// ticket in it; the page's MediaSession attaches to it (rcdp wire v2), so the
// video never passes through Electron IPC.
//
// The call is gRPC-Web over HTTP/1.1 on the daemon's loopback listener, which
// accepts it next to native gRPC, with the bearer token from
// `$CUA_HOME/daemon.json`. Two strings go in and four fields come out, so the
// protobuf is encoded by hand here instead of pulling in a gRPC stack. No
// Electron in this file: `test/media-bridge.test.ts` drives it with a fake
// fetch.
import { readFile } from "node:fs/promises";
import * as path from "node:path";
import type { StreamTarget, StreamTicket, StreamTier } from "../../cua-spaces-web/src/bridge/ops/stream";

/** `cua_daemon::Discovery`, the parts this needs. */
export interface DaemonEndpoint {
  loopbackUrl: string;
  token: string;
}

/** Reads `daemon.json`; null when there is none or it has no loopback listener. */
export async function readDaemonEndpoint(cuaHome: string, read = (p: string) => readFile(p, "utf8")): Promise<DaemonEndpoint | null> {
  let raw: unknown;
  try {
    raw = JSON.parse(await read(path.join(cuaHome, "daemon.json")));
  } catch {
    return null;
  }
  const d = raw as { loopback_url?: unknown; token?: unknown };
  if (typeof d.loopback_url !== "string" || !/^http:\/\/(127\.0\.0\.1|\[::1\]|localhost):\d+\/?$/.test(d.loopback_url)) return null;
  if (typeof d.token !== "string" || d.token === "") return null;
  return { loopbackUrl: d.loopback_url.replace(/\/$/, ""), token: d.token };
}

/**
 * The `cua.env.v1.OpenMediaRequest` for a tier, in canonical proto3 JSON.
 * Tiles match the macOS app's `VideoTier.tile` (10 fps, 960 px long edge,
 * view only); the viewer takes the Space's defaults and input, like the Tauri
 * app's `open_media_request`. One window (picture in picture) takes its
 * input in the background, as the Swift app's
 * `SpaceStreamProvider.inputPolicy`, unless `activate` (the Space said that
 * window needs activating).
 */
export function openMediaRequest(tier: StreamTier, window?: Pick<StreamTarget, "windowId" | "epoch" | "activate">): Record<string, unknown> {
  const target = window?.windowId ? { window: { id: window.windowId, epoch: String(window.epoch ?? 1) } } : { displayId: "primary" };
  const common = { target, geometryControl: "GEOMETRY_CONTROL_OBSERVE_ONLY", ticketTtl: "120s" };
  if (tier === "tile") return { ...common, maxFps: 10, maxDimension: 960, policy: "SESSION_POLICY_VIEW_ONLY" };
  const background = window?.windowId && !window.activate;
  return { ...common, policy: background ? "SESSION_POLICY_BACKGROUND_ONLY" : "SESSION_POLICY_ALLOW_ACTIVATION" };
}

/* ---- protobuf, the two messages ----------------------------------------- */

function varint(n: number): number[] {
  const out: number[] = [];
  let v = n;
  while (v > 0x7f) {
    out.push((v & 0x7f) | 0x80);
    v = Math.floor(v / 128);
  }
  out.push(v);
  return out;
}

function stringField(field: number, value: string): number[] {
  const bytes = [...new TextEncoder().encode(value)];
  return bytes.length === 0 ? [] : [(field << 3) | 2, ...varint(bytes.length), ...bytes];
}

/** `OpenMediaBridgeRequest { name = 1; open_media_json = 2; }`. */
export function encodeOpenMediaBridgeRequest(name: string, openMediaJson: string): Uint8Array {
  return Uint8Array.from([...stringField(1, name), ...stringField(2, openMediaJson)]);
}

class Reader {
  pos = 0;
  constructor(readonly buf: Uint8Array) {}
  get done() {
    return this.pos >= this.buf.length;
  }
  varint(): number {
    let result = 0;
    let scale = 1;
    for (;;) {
      if (this.pos >= this.buf.length) throw new Error("truncated varint");
      const b = this.buf[this.pos++]!;
      result += (b & 0x7f) * scale;
      if (b < 0x80) return result;
      scale *= 128;
    }
  }
  bytes(): Uint8Array {
    const n = this.varint();
    if (this.pos + n > this.buf.length) throw new Error("truncated field");
    const out = this.buf.subarray(this.pos, this.pos + n);
    this.pos += n;
    return out;
  }
  skip(wire: number): void {
    if (wire === 0) this.varint();
    else if (wire === 2) this.bytes();
    else if (wire === 1) this.pos += 8;
    else if (wire === 5) this.pos += 4;
    else throw new Error(`unknown wire type ${wire}`);
  }
}

export interface OpenMediaBridgeResponse {
  wsUrl: string;
  ticket: string;
  /** Unix seconds, or null when unset. */
  expiresAt: number | null;
  openMediaResponseJson: string;
}

/** `OpenMediaBridgeResponse { ws_url = 1; ticket = 2; Timestamp expires_at = 3; open_media_response_json = 4; }`. */
export function decodeOpenMediaBridgeResponse(buf: Uint8Array): OpenMediaBridgeResponse {
  const text = (b: Uint8Array) => new TextDecoder().decode(b);
  const out: OpenMediaBridgeResponse = { wsUrl: "", ticket: "", expiresAt: null, openMediaResponseJson: "" };
  const r = new Reader(buf);
  while (!r.done) {
    const tag = r.varint();
    const field = Math.floor(tag / 8);
    const wire = tag & 7;
    if (wire !== 2) {
      r.skip(wire);
      continue;
    }
    const value = r.bytes();
    if (field === 1) out.wsUrl = text(value);
    else if (field === 2) out.ticket = text(value);
    else if (field === 4) out.openMediaResponseJson = text(value);
    else if (field === 3) {
      // google.protobuf.Timestamp { int64 seconds = 1; int32 nanos = 2; }
      const ts = new Reader(value);
      let seconds = 0;
      while (!ts.done) {
        const t = ts.varint();
        if (t === 8) seconds = ts.varint();
        else ts.skip(t & 7);
      }
      out.expiresAt = seconds;
    }
  }
  return out;
}

/* ---- gRPC-Web ------------------------------------------------------------ */

export class DaemonCallError extends Error {
  constructor(
    message: string,
    readonly grpcStatus: number | null,
  ) {
    super(message);
    this.name = "DaemonCallError";
  }
}

export function grpcWebFrame(message: Uint8Array): Uint8Array {
  const out = new Uint8Array(5 + message.length);
  new DataView(out.buffer).setUint32(1, message.length);
  out.set(message, 5);
  return out;
}

/** The message and status of a unary gRPC-Web response (data frame, then a trailer frame, or a trailers-only reply in the headers). */
export function parseGrpcWebResponse(body: Uint8Array, headers: { get(name: string): string | null }): { message: Uint8Array | null; status: number; statusMessage: string } {
  let message: Uint8Array | null = null;
  const trailers = new Map<string, string>();
  for (let pos = 0; pos + 5 <= body.length; ) {
    const flag = body[pos]!;
    const len = new DataView(body.buffer, body.byteOffset + pos + 1, 4).getUint32(0);
    const frame = body.subarray(pos + 5, pos + 5 + len);
    pos += 5 + len;
    if (flag & 0x80) {
      for (const line of new TextDecoder().decode(frame).split("\r\n")) {
        const i = line.indexOf(":");
        if (i > 0) trailers.set(line.slice(0, i).trim().toLowerCase(), line.slice(i + 1).trim());
      }
    } else if (message === null) message = frame;
  }
  const raw = trailers.get("grpc-status") ?? headers.get("grpc-status");
  const statusMessage = trailers.get("grpc-message") ?? headers.get("grpc-message") ?? "";
  let decoded = statusMessage;
  try {
    decoded = decodeURIComponent(statusMessage);
  } catch {
    // keep it as sent
  }
  return { message, status: raw == null ? -1 : Number(raw), statusMessage: decoded };
}

export type Fetch = (url: string, init: { method: string; headers: Record<string, string>; body: Uint8Array; signal?: AbortSignal }) => Promise<{
  ok: boolean;
  status: number;
  headers: { get(name: string): string | null };
  arrayBuffer(): Promise<ArrayBuffer>;
}>;

/** One `DaemonService.OpenMediaBridge` call. */
export async function openMediaBridge(endpoint: DaemonEndpoint, name: string, openMediaJson: string, fetchImpl: Fetch, timeoutMs = 15_000): Promise<OpenMediaBridgeResponse> {
  const res = await fetchImpl(`${endpoint.loopbackUrl}/cua.daemon.v1.DaemonService/OpenMediaBridge`, {
    method: "POST",
    headers: {
      "content-type": "application/grpc-web+proto",
      accept: "application/grpc-web+proto",
      "x-grpc-web": "1",
      authorization: `Bearer ${endpoint.token}`,
    },
    body: grpcWebFrame(encodeOpenMediaBridgeRequest(name, openMediaJson)),
    signal: AbortSignal.timeout(timeoutMs),
  });
  if (res.status === 401) throw new DaemonCallError("The cua daemon refused the token in daemon.json.", 16);
  if (!res.ok) throw new DaemonCallError(`The cua daemon answered HTTP ${res.status}.`, null);
  const { message, status, statusMessage } = parseGrpcWebResponse(new Uint8Array(await res.arrayBuffer()), res.headers);
  if (status !== 0) throw new DaemonCallError(statusMessage || `The cua daemon failed the call (status ${status}).`, status);
  if (!message) throw new DaemonCallError("The cua daemon answered without a message.", status);
  const out = decodeOpenMediaBridgeResponse(message);
  if (!/^wss?:\/\//.test(out.wsUrl)) throw new DaemonCallError("The cua daemon answered without a media URL.", status);
  return out;
}

/* ---- the operation ------------------------------------------------------- */

export interface StreamDeps {
  cuaHome: string;
  fetch: Fetch;
  /** Whether this app bundles cua (it starts the daemon with it). */
  hasCua: () => Promise<boolean>;
  /** `cua daemon start`. */
  startDaemon: () => Promise<void>;
  readEndpoint?: (cuaHome: string) => Promise<DaemonEndpoint | null>;
  sleep?: (ms: number) => Promise<void>;
  /**
   * Tests and the WebCodecs harness: answer every request with this URL
   * (`{id}` and `{tier}` filled in) instead of asking the daemon
   * (`CUA_SPACES_TEST_STREAM_URL`; ignored in packaged builds unless set).
   */
  testStreamUrl?: string | null;
}

export class StreamUnavailableError extends Error {
  readonly code = "unsupported";
  constructor(message: string) {
    super(message);
    this.name = "StreamUnavailableError";
  }
}

/** `spaces.openStream {spaceId, tier, windowId?, epoch?, activate?}` → `{wsUrl, expiresAt}`. Starts the daemon when it isn't running. */
export function createStreamOpener(deps: StreamDeps) {
  const readEndpoint = deps.readEndpoint ?? ((home: string) => readDaemonEndpoint(home));
  const sleep = deps.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));

  async function endpoint(): Promise<DaemonEndpoint> {
    const found = await readEndpoint(deps.cuaHome);
    if (found) return found;
    await deps.startDaemon();
    for (let i = 0; i < 40; i++) {
      await sleep(250);
      const e = await readEndpoint(deps.cuaHome);
      if (e) return e;
    }
    // Not "no video here": the stream failed to open, and Try again asks again
    // (the SwiftUI app's detail when `streamProvider` throws).
    throw new Error("The cua daemon didn't start, so live video can't open.");
  }

  return async function openStream({ spaceId, tier, ...window }: StreamTarget): Promise<StreamTicket> {
    if (typeof spaceId !== "string" || spaceId === "") throw new Error("spaceId is required");
    if (tier !== "tile" && tier !== "full") throw new Error("tier must be tile or full");
    if (deps.testStreamUrl) {
      const wsUrl = deps.testStreamUrl.replaceAll("{id}", encodeURIComponent(spaceId)).replaceAll("{tier}", tier);
      return { wsUrl, expiresAt: null };
    }
    if (!(await deps.hasCua())) throw new StreamUnavailableError("Live video needs cua on this computer.");
    const json = JSON.stringify(openMediaRequest(tier, window));
    let e = await endpoint();
    try {
      const r = await openMediaBridge(e, spaceId, json, deps.fetch);
      return { wsUrl: r.wsUrl, expiresAt: r.expiresAt === null ? null : new Date(r.expiresAt * 1000).toISOString() };
    } catch (err) {
      // A connection refused means daemon.json outlived its daemon: start one and try once more.
      if (err instanceof DaemonCallError) throw err;
      await deps.startDaemon();
      e = await endpoint();
      const r = await openMediaBridge(e, spaceId, json, deps.fetch);
      return { wsUrl: r.wsUrl, expiresAt: r.expiresAt === null ? null : new Date(r.expiresAt * 1000).toISOString() };
    }
  };
}
