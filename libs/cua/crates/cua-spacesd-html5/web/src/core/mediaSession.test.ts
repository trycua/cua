// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { commandAsControl, MediaSession, type FrameTiming, type MediaSessionOptions, type MediaStatus } from "./mediaSession";
import { encodeAudioPacket, encodeVideoPacket, type VideoDescriptor } from "./mediaWire";

/**
 * In-memory WebSocket stand-in. Records every socket the session opens and
 * every text frame it sends; tests drive server frames and closes back.
 */
class MockSocket {
  static OPEN = 1;
  static CLOSED = 3;
  static all: MockSocket[] = [];
  readyState = MockSocket.OPEN;
  binaryType = "";
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: ((event: { code: number }) => void) | null = null;
  onerror: (() => void) | null = null;
  readonly sent: Array<{ type: string; payload?: Record<string, unknown> }> = [];
  constructor(
    public readonly url: string,
    public readonly protocols?: string[],
  ) {
    MockSocket.all.push(this);
  }
  send(data: string): void {
    this.sent.push(JSON.parse(data));
  }
  close(): void {
    this.readyState = MockSocket.CLOSED;
  }
  server(type: string, payload?: unknown): void {
    this.onmessage?.({ data: JSON.stringify({ type, payload }) });
  }
  binary(buffer: ArrayBuffer): void {
    this.onmessage?.({ data: buffer });
  }
  drop(code: number): void {
    this.readyState = MockSocket.CLOSED;
    this.onclose?.({ code });
  }
  of(type: string) {
    return this.sent.filter((m) => m.type === type);
  }
  static get last(): MockSocket {
    return MockSocket.all[MockSocket.all.length - 1]!;
  }
}

const KEYFRAME = new Uint8Array([0, 0, 0, 1, 0x67, 0x42, 0xe0, 0x1f, 0xff]);
const DELTA = new Uint8Array([0, 0, 0, 1, 0x41, 0x9a, 0x00]);

function frame(overrides: Partial<VideoDescriptor>, payload: Uint8Array): ArrayBuffer {
  return encodeVideoPacket(
    {
      session_id: "m-1",
      sequence: 900_001,
      geometry_epoch: 1,
      codec_epoch: 1,
      width_px: 800,
      height_px: 600,
      capture_timestamp_us: 1_000,
      codec: "h264",
      keyframe: false,
      ...overrides,
    },
    payload,
  );
}

type DecoderEntry = {
  configured: boolean;
  closed: boolean;
  decoded: number;
  fail: () => void;
  config?: Record<string, unknown>;
  /** Hands out a decoded frame for the chunk with this timestamp. */
  emit: (timestamp: number) => void;
};
let decoders: DecoderEntry[];
const saved: Record<string, unknown> = {};

beforeEach(() => {
  vi.useFakeTimers();
  MockSocket.all = [];
  for (const k of ["WebSocket", "VideoDecoder", "EncodedVideoChunk"]) saved[k] = (globalThis as Record<string, unknown>)[k];
  (globalThis as Record<string, unknown>).WebSocket = MockSocket;
  decoders = [];
  (globalThis as Record<string, unknown>).VideoDecoder = class {
    state = "configured";
    decodeQueueSize = 0;
    private readonly entry: DecoderEntry = { configured: false, closed: false, decoded: 0, fail: () => {}, emit: () => {} };
    constructor(init: { error: () => void; output: (frame: unknown) => void }) {
      this.entry.fail = () => init.error();
      this.entry.emit = (timestamp) => init.output({ timestamp, displayWidth: 800, displayHeight: 600, close() {} });
      decoders.push(this.entry);
    }
    configure(config: Record<string, unknown>) {
      this.entry.configured = true;
      this.entry.config = config;
    }
    decode() {
      this.entry.decoded += 1;
    }
    close() {
      this.entry.closed = true;
      this.state = "closed";
    }
  };
  (globalThis as Record<string, unknown>).EncodedVideoChunk = class {
    constructor(public readonly init: unknown) {}
  };
  vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue(null as unknown as CanvasRenderingContext2D);
});

afterEach(() => {
  for (const [k, v] of Object.entries(saved)) (globalThis as Record<string, unknown>)[k] = v;
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function start(overrides: Partial<MediaSessionOptions> = {}) {
  const statuses: Array<[MediaStatus, string | undefined]> = [];
  const canvas = document.createElement("canvas");
  const session = new MediaSession({
    canvas,
    ticket: { wsUrl: "ws://10.0.0.5:3211/media?ticket=t1", ticketExpiresAt: "2999-01-01T00:00:00Z" },
    interactive: true,
    audio: false,
    onStatus: (s, d) => statuses.push([s, d]),
    ...overrides,
  });
  session.start();
  return { session, canvas, statuses, socket: MockSocket.last };
}

function handshake(socket: MockSocket, extra: Record<string, unknown> = {}) {
  socket.server("hello", {
    protocol: "rcdp",
    versions: [2],
    selected_version: 2,
    capabilities: ["input.interactive.v2", "desktop.v1", "keyframe_on_attach.v1", "audio.v1"],
  });
  socket.server("session_opened", {
    session_id: "m-1",
    target: { kind: "display", display_id: "primary" },
    geometry: { width_px: 1280, height_px: 800, scale_factor: 1 },
    geometry_epoch: 1,
    codec: "h264",
    policy: "allow_activation",
    geometry_control: "observe_only",
    ...extra,
  });
}

describe("MediaSession attach + handshake", () => {
  it("attaches the ticketed URL with the rcdp.v2 subprotocol and never speaks first", () => {
    const { socket, session, canvas, statuses } = start();
    expect(socket.url).toBe("ws://10.0.0.5:3211/media?ticket=t1");
    expect(socket.protocols).toEqual(["rcdp.v2"]);
    expect(socket.sent).toHaveLength(0);
    handshake(socket);
    expect(session.id).toBe("m-1");
    expect(session.serverCapabilities).toContain("keyframe_on_attach.v1");
    expect(statuses.at(-1)?.[0]).toBe("streaming");
    expect([canvas.width, canvas.height]).toEqual([1280, 800]);
    // No authenticate, no hello, no open_session: v2 is server-first.
    expect(socket.sent).toHaveLength(0);
    session.stop();
  });

  it("follows lifecycle geometry_changed as the authoritative frame size", () => {
    const onGeometry = vi.fn();
    const { socket, canvas } = start({ onGeometry });
    handshake(socket);
    socket.server("lifecycle", {
      session_id: "m-1",
      event: { kind: "geometry_changed", geometry_epoch: 2, geometry: { width_px: 640, height_px: 400, scale_factor: 1 } },
    });
    expect(onGeometry).toHaveBeenLastCalledWith(640, 400);
    expect([canvas.width, canvas.height]).toEqual([640, 400]);
  });
});

describe("MediaSession video (keyframe on attach, epochs)", () => {
  it("decodes from the first packet (a keyframe) whatever its sequence, without asking", () => {
    const { socket, session } = start();
    handshake(socket);
    socket.binary(frame({ keyframe: true, sequence: 3_000_000_000 }, KEYFRAME));
    socket.binary(frame({ sequence: 3_000_000_004, capture_timestamp_us: 2_000 }, DELTA));
    expect(decoders).toHaveLength(1);
    expect(decoders[0]!.decoded).toBe(2);
    expect(socket.of("request_keyframe")).toHaveLength(0);
    expect(session.lastFrameSequence).toBe(3_000_000_004);
  });

  it("rebuilds the decoder on a codec_epoch change from the next keyframe", () => {
    const { socket } = start();
    handshake(socket);
    socket.binary(frame({ keyframe: true }, KEYFRAME));
    socket.binary(frame({ codec_epoch: 2, sequence: 900_002 }, DELTA)); // no IDR yet
    expect(socket.of("request_keyframe")).toHaveLength(1);
    socket.binary(frame({ codec_epoch: 2, keyframe: true, sequence: 900_003 }, KEYFRAME));
    expect(decoders).toHaveLength(2);
    expect(decoders[0]!.closed).toBe(true);
    expect(decoders[1]!.decoded).toBe(1);
  });

  it("drops a failed decoder, asks for an IDR (throttled to 1/s) and rebuilds", () => {
    const { socket } = start();
    handshake(socket);
    socket.binary(frame({ keyframe: true }, KEYFRAME));
    decoders[0]!.fail();
    expect(decoders[0]!.closed).toBe(true);
    for (let i = 0; i < 5; i++) socket.binary(frame({ sequence: 900_010 + i }, DELTA));
    expect(socket.of("request_keyframe")).toHaveLength(1);
    expect(socket.of("request_keyframe")[0]!.payload).toEqual({ session_id: "m-1" });
    vi.advanceTimersByTime(1_100);
    socket.binary(frame({ sequence: 900_020 }, DELTA));
    expect(socket.of("request_keyframe")).toHaveLength(2);
    socket.binary(frame({ keyframe: true, sequence: 900_021 }, KEYFRAME));
    expect(decoders).toHaveLength(2);
  });

  it("ignores frames for another session and tolerates audio with audio off", () => {
    const { socket } = start();
    handshake(socket);
    socket.binary(frame({ keyframe: true, session_id: "other" }, KEYFRAME));
    socket.binary(encodeAudioPacket({ trackId: 1, sequence: 1, ptsUs: 0, frameSamples: 960, configEpoch: 0 }, new Uint8Array(8)));
    expect(decoders).toHaveLength(0);
  });
});

describe("MediaSession interactive input (§6)", () => {
  it("batches events under the session id with an adopted base, then continues contiguously", async () => {
    const { socket, session } = start();
    handshake(socket);
    session.queueInput({ kind: "text_commit", text: "h" });
    session.queueInput({ kind: "text_commit", text: "i" });
    session.queueInput({ kind: "key", key: "enter", state: "down", modifiers: [], repeat: false });
    await Promise.resolve();
    const [first] = socket.of("interactive_input");
    expect(first!.payload!.session_id).toBe("m-1");
    const base = first!.payload!.first_sequence as number;
    expect(base).toBeGreaterThan(0);
    expect((first!.payload!.events as unknown[]).length).toBe(2); // "hi" coalesced + enter
    session.queueInput({ kind: "text_commit", text: "!" });
    await Promise.resolve();
    expect(socket.of("interactive_input")[1]!.payload!.first_sequence).toBe(base + 2);
  });

  it("re-syncs to expected_sequence after input_sequence_gap", async () => {
    const { socket, session } = start();
    handshake(socket);
    socket.server("error", { code: "input_sequence_gap", message: "gap", expected_sequence: 77 });
    session.queueInput({ kind: "text_commit", text: "x" });
    await Promise.resolve();
    expect(socket.of("interactive_input")[0]!.payload!.first_sequence).toBe(77);
  });

  it("maps pointer positions over the video, not the letterboxed canvas box", async () => {
    const { socket, canvas } = start();
    handshake(socket);
    // A 1280x800 desktop in a 956x284 box under `object-fit: contain`: the video is 454.4 wide, centred.
    canvas.width = 1280;
    canvas.height = 800;
    vi.spyOn(window, "getComputedStyle").mockReturnValue({ objectFit: "contain" } as CSSStyleDeclaration);
    vi.spyOn(canvas, "getBoundingClientRect").mockReturnValue({ left: 593, top: 323, width: 956, height: 284, right: 1549, bottom: 607, x: 593, y: 323, toJSON: () => ({}) });
    const press = (clientX: number, clientY: number) => canvas.dispatchEvent(new MouseEvent("pointerdown", { clientX, clientY, button: 0 }));
    press(1071, 465); // the middle of the box
    press(843.8 + 454.4 * 0.25, 323 + 284 * 0.5); // a quarter of the way across the video
    press(700, 323); // in the left bar: pinned to the video's left edge
    await Promise.resolve();
    const events = socket.of("interactive_input").flatMap((m) => m.payload!.events as Array<{ x_normalized: number; y_normalized: number }>);
    expect(events).toHaveLength(3);
    expect(events[0]!.x_normalized).toBeCloseTo(0.5, 2);
    expect(events[0]!.y_normalized).toBeCloseTo(0.5, 2);
    expect(events[1]!.x_normalized).toBeCloseTo(0.25, 2);
    expect(events[2]!.x_normalized).toBe(0);
  });

  it("sends no input on a view_only session or a non-interactive one", async () => {
    const viewOnly = start();
    handshake(viewOnly.socket, { policy: "view_only" });
    viewOnly.session.queueInput({ kind: "text_commit", text: "x" });
    await Promise.resolve();
    expect(viewOnly.socket.of("interactive_input")).toHaveLength(0);

    const pip = start({ interactive: false });
    handshake(pip.socket);
    pip.session.queueInput({ kind: "text_commit", text: "x" });
    await Promise.resolve();
    expect(pip.socket.of("interactive_input")).toHaveLength(0);
  });

  it("sends set_window_geometry only with a bidirectional grant, revisions increasing", () => {
    const observe = start();
    handshake(observe.socket);
    expect(observe.session.setWindowGeometry(800, 600)).toBe(false);

    const owner = start();
    handshake(owner.socket, { geometry_control: "bidirectional" });
    expect(owner.session.setWindowGeometry(800, 600)).toBe(true);
    owner.session.setWindowGeometry(820, 610);
    const sent = owner.socket.of("set_window_geometry").map((m) => m.payload!.revision);
    expect(sent).toEqual([1, 2]);
  });
});

describe("MediaSession disconnects and tickets (§2, §10)", () => {
  it("re-attaches with the SAME ticket after a network drop while it is valid", () => {
    const reopen = vi.fn();
    const { socket, statuses } = start({ reopen });
    handshake(socket);
    socket.drop(1006);
    expect(statuses.at(-1)?.[0]).toBe("reconnecting");
    vi.advanceTimersByTime(1_600);
    expect(MockSocket.all).toHaveLength(2);
    expect(MockSocket.last.url).toBe("ws://10.0.0.5:3211/media?ticket=t1");
    expect(reopen).not.toHaveBeenCalled();
  });

  it("mints a new ticket on 4401 and on expiry", async () => {
    const reopen = vi.fn(async () => ({ wsUrl: "ws://10.0.0.5:3211/media?ticket=t2" }));
    const { socket } = start({ reopen });
    handshake(socket);
    socket.drop(4401);
    await vi.advanceTimersByTimeAsync(1_600);
    expect(reopen).toHaveBeenCalledTimes(1);
    expect(MockSocket.last.url).toBe("ws://10.0.0.5:3211/media?ticket=t2");

    const expired = start({
      reopen,
      ticket: { wsUrl: "ws://h/media?ticket=old", ticketExpiresAt: "2000-01-01T00:00:00Z" },
    });
    expired.socket.drop(1006);
    await vi.advanceTimersByTimeAsync(1_600);
    expect(reopen).toHaveBeenCalledTimes(2);
  });

  it("ends for good on 4404 / 4410 and does not reconnect", () => {
    const onGone = vi.fn();
    const { socket, statuses } = start({ onGone });
    handshake(socket);
    socket.drop(4410);
    vi.advanceTimersByTime(10_000);
    expect(MockSocket.all).toHaveLength(1);
    expect(statuses.at(-1)?.[0]).toBe("ended");
    expect(onGone).toHaveBeenCalledWith("the window is gone");
  });

  it("ends a borrowed ticket (no reopen) instead of minting one", async () => {
    const onGone = vi.fn();
    const { socket } = start({ onGone, reopen: undefined });
    socket.drop(4401);
    await vi.advanceTimersByTimeAsync(1_600);
    expect(onGone).toHaveBeenCalledWith("ticket expired");
    expect(MockSocket.all).toHaveLength(1);
  });

  it("gives up after a bounded number of failed attaches", async () => {
    const { socket, statuses } = start();
    socket.drop(1006);
    for (let i = 0; i < 25; i++) {
      await vi.advanceTimersByTimeAsync(1_600);
      MockSocket.last.drop(1006);
    }
    expect(statuses.at(-1)?.[0]).toBe("failed");
    expect(MockSocket.all.length).toBeLessThanOrEqual(22);
  });

  it("stop() closes the socket and never reconnects", () => {
    const { socket, session } = start();
    session.stop();
    expect(socket.readyState).toBe(MockSocket.CLOSED);
    vi.advanceTimersByTime(10_000);
    expect(MockSocket.all).toHaveLength(1);
  });
});

describe("MediaSession in the Electron shell (hardware decode, keys, scrolls, timing)", () => {
  it("asks for the hardware decoder at low latency, and decodes in software once it failed before a frame", () => {
    const { socket } = start({ hardwareAcceleration: "prefer-hardware" });
    handshake(socket);
    socket.binary(frame({ keyframe: true }, KEYFRAME));
    expect(decoders[0]!.config).toMatchObject({ optimizeForLatency: true, hardwareAcceleration: "prefer-hardware" });
    decoders[0]!.fail();
    socket.binary(frame({ keyframe: true, sequence: 900_002 }, KEYFRAME));
    expect(decoders[1]!.config).toMatchObject({ optimizeForLatency: true, hardwareAcceleration: "no-preference" });
  });

  it("keeps the hardware decoder after a failure once it had drawn a frame", () => {
    const { socket } = start({ hardwareAcceleration: "prefer-hardware" });
    handshake(socket);
    socket.binary(frame({ keyframe: true, capture_timestamp_us: 1_000 }, KEYFRAME));
    decoders[0]!.emit(1_000);
    decoders[0]!.fail();
    socket.binary(frame({ keyframe: true, sequence: 900_002 }, KEYFRAME));
    expect(decoders[1]!.config).toMatchObject({ hardwareAcceleration: "prefer-hardware" });
  });

  it("decodes with no preference by default (the HTML5 viewer)", () => {
    const { socket } = start();
    handshake(socket);
    socket.binary(frame({ keyframe: true }, KEYFRAME));
    expect(decoders[0]!.config).toMatchObject({ hardwareAcceleration: "no-preference" });
  });

  it("times each drawn frame from its packet's arrival", () => {
    const timings: FrameTiming[] = [];
    const { socket } = start({ onFrameTiming: (t) => timings.push(t) });
    handshake(socket);
    vi.setSystemTime(10_000);
    socket.binary(frame({ keyframe: true, capture_timestamp_us: 5_000 }, KEYFRAME));
    vi.advanceTimersByTime(12);
    decoders[0]!.emit(5_000);
    expect(timings).toHaveLength(1);
    expect(timings[0]!.drawnAt - timings[0]!.receivedAt).toBeCloseTo(12, 0);
  });

  it("sends a Mac's command as Control to a Linux or Windows guest", () => {
    const { socket, canvas } = start({ metaAsControl: true });
    handshake(socket);
    canvas.dispatchEvent(new KeyboardEvent("keydown", { key: "c", metaKey: true }));
    canvas.dispatchEvent(new KeyboardEvent("keydown", { key: "C", metaKey: true, shiftKey: true }));
    canvas.dispatchEvent(new KeyboardEvent("keydown", { key: "a" }));
    vi.runAllTicks();
    return Promise.resolve().then(() => {
      const events = socket.of("interactive_input").flatMap((m) => m.payload!.events as Array<Record<string, unknown>>);
      expect(events).toEqual([
        { kind: "key", key: "c", state: "down", modifiers: ["control"], repeat: false },
        { kind: "key", key: "c", state: "down", modifiers: ["shift", "control"], repeat: false },
        { kind: "text_commit", text: "a" },
      ]);
      expect(commandAsControl({ kind: "key", key: "meta", state: "down", modifiers: ["command"], repeat: false })).toEqual({
        kind: "key",
        key: "control",
        state: "down",
        modifiers: ["control"],
        repeat: false,
      });
    });
  });

  it("sends a ⌘ chord whole from a Mac, whose key-up Chromium never fires", async () => {
    const { socket, canvas } = start({ wholeCommandChords: true });
    handshake(socket);
    canvas.dispatchEvent(new KeyboardEvent("keydown", { key: "Meta", metaKey: true }));
    canvas.dispatchEvent(new KeyboardEvent("keydown", { key: "c", metaKey: true }));
    canvas.dispatchEvent(new KeyboardEvent("keydown", { key: "x" }));
    await Promise.resolve();
    const events = socket.of("interactive_input").flatMap((m) => m.payload!.events as Array<Record<string, unknown>>);
    expect(events).toEqual([
      { kind: "key", key: "meta", state: "down", modifiers: ["command"], repeat: false },
      { kind: "key", key: "c", state: "down", modifiers: ["command"], repeat: false },
      { kind: "key", key: "c", state: "up", modifiers: ["command"], repeat: false },
      { kind: "text_commit", text: "x" },
    ]);
  });

  it("lets scrolls go to the page until the canvas has the keyboard", async () => {
    const { socket, canvas } = start({ scrollNeedsFocus: true });
    document.body.append(canvas);
    handshake(socket);
    const before = new WheelEvent("wheel", { deltaY: 40, cancelable: true });
    canvas.dispatchEvent(before);
    expect(before.defaultPrevented).toBe(false);
    canvas.focus();
    const after = new WheelEvent("wheel", { deltaY: 40, cancelable: true });
    canvas.dispatchEvent(after);
    expect(after.defaultPrevented).toBe(true);
    await Promise.resolve();
    expect(socket.of("interactive_input").flatMap((m) => m.payload!.events as Array<{ kind: string }>).map((e) => e.kind)).toEqual(["scroll"]);
    canvas.remove();
  });

  it("hands input acknowledgements to the caller", () => {
    const acks: Record<string, unknown>[] = [];
    const { socket } = start({ onInputAck: (a) => acks.push(a) });
    handshake(socket);
    socket.server("interactive_input_acknowledgement", { through_sequence: 4, delivered: false, error: { code: "would_require_activation" } });
    expect(acks).toEqual([{ through_sequence: 4, delivered: false, error: { code: "would_require_activation" } }]);
  });
});
