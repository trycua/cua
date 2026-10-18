// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Demonstration recording for `cua skills record`: the canvas is recorded
 * with MediaRecorder (MP4 where the engine can, else WebM) while the
 * viewer's own input events are logged with timestamps. On stop, the
 * recording goes to `record_url` (a loopback WebSocket the CLI serves) as
 * `[u32 BE JSON length][JSON {events, metadata}][video bytes]`, in 1 MiB
 * binary messages.
 */

import type { InteractiveInputEvent } from "./core/mediaWire";

export interface RecordedEvent {
  type: "click" | "double_click" | "right_click" | "drag" | "type" | "key" | "scroll";
  /** Milliseconds since the recording started. */
  timestamp: number;
  x?: number;
  y?: number;
  to_x?: number;
  to_y?: number;
  button?: string;
  text?: string;
  key?: string;
  modifiers?: string[];
  delta_x?: number;
  delta_y?: number;
}

/** Turns raw input events into the steps a skill is captioned from. */
export class ActionLog {
  readonly events: RecordedEvent[] = [];
  private down: { x: number; y: number; button: string; at: number } | null = null;
  private lastClick: { x: number; y: number; at: number } | null = null;

  constructor(
    private readonly start: number,
    private readonly size: () => { width: number; height: number },
  ) {}

  private px(nx: number, ny: number): { x: number; y: number } {
    const { width, height } = this.size();
    return { x: Math.round(nx * width), y: Math.round(ny * height) };
  }

  add(event: InteractiveInputEvent, now: number): void {
    const timestamp = Math.max(0, Math.round(now - this.start));
    switch (event.kind) {
      case "pointer": {
        const at = this.px(event.x_normalized, event.y_normalized);
        if (event.phase === "down") {
          this.down = { ...at, button: event.button ?? "left", at: timestamp };
        } else if (event.phase === "up" && this.down) {
          const start = this.down;
          this.down = null;
          const moved = Math.hypot(at.x - start.x, at.y - start.y) > 4;
          if (moved) {
            this.events.push({ type: "drag", timestamp: start.at, x: start.x, y: start.y, to_x: at.x, to_y: at.y, button: start.button });
          } else if (start.button === "right") {
            this.events.push({ type: "right_click", timestamp: start.at, ...at, button: "right" });
          } else {
            const prev = this.lastClick;
            const last = this.events[this.events.length - 1];
            if (prev && start.at - prev.at < 400 && Math.hypot(prev.x - at.x, prev.y - at.y) < 6 && last?.type === "click") {
              last.type = "double_click";
              this.lastClick = null;
            } else {
              this.events.push({ type: "click", timestamp: start.at, ...at, button: start.button });
              this.lastClick = { ...at, at: start.at };
            }
          }
        }
        break;
      }
      case "text_commit": {
        const last = this.events[this.events.length - 1];
        if (last?.type === "type" && timestamp - last.timestamp < 1500) last.text = (last.text ?? "") + event.text;
        else this.events.push({ type: "type", timestamp, text: event.text });
        break;
      }
      case "key":
        if (event.state === "down" && !["shift", "control", "alt", "meta"].includes(event.key)) {
          this.events.push({ type: "key", timestamp, key: event.key, modifiers: event.modifiers });
        }
        break;
      case "scroll": {
        const last = this.events[this.events.length - 1];
        const at = this.px(event.x_normalized, event.y_normalized);
        if (last?.type === "scroll" && timestamp - last.timestamp < 500) {
          last.delta_x = (last.delta_x ?? 0) + event.delta_x;
          last.delta_y = (last.delta_y ?? 0) + event.delta_y;
        } else {
          this.events.push({ type: "scroll", timestamp, ...at, delta_x: event.delta_x, delta_y: event.delta_y });
        }
        break;
      }
    }
  }
}

/** Frames `[u32 BE json length][json][video]`. */
export function frameRecording(meta: unknown, video: Uint8Array): Uint8Array {
  const json = new TextEncoder().encode(JSON.stringify(meta));
  const out = new Uint8Array(4 + json.byteLength + video.byteLength);
  new DataView(out.buffer).setUint32(0, json.byteLength, false);
  out.set(json, 4);
  out.set(video, 4 + json.byteLength);
  return out;
}

export function recorderMime(preferMp4: boolean): string | undefined {
  if (typeof MediaRecorder === "undefined") return undefined;
  const options = preferMp4
    ? ["video/mp4;codecs=avc1", "video/mp4", "video/webm;codecs=vp9", "video/webm"]
    : ["video/webm;codecs=vp9", "video/webm", "video/mp4"];
  return options.find((type) => MediaRecorder.isTypeSupported(type));
}

export class SkillRecorder {
  private recorder: MediaRecorder | null = null;
  private readonly chunks: Blob[] = [];
  readonly log: ActionLog;
  private readonly started = performance.now();

  constructor(
    private readonly canvas: HTMLCanvasElement,
    private readonly url: string,
    preferMp4: boolean,
  ) {
    this.log = new ActionLog(this.started, () => ({ width: canvas.width, height: canvas.height }));
    const mime = recorderMime(preferMp4);
    const stream = canvas.captureStream(30);
    this.recorder = new MediaRecorder(stream, mime ? { mimeType: mime, videoBitsPerSecond: 4_000_000 } : undefined);
    this.recorder.ondataavailable = (event) => {
      if (event.data.size > 0) this.chunks.push(event.data);
    };
    this.recorder.start(1000);
  }

  add(event: InteractiveInputEvent): void {
    this.log.add(event, performance.now());
  }

  /** Stops and uploads. Resolves when the CLI has the whole recording. */
  async finish(): Promise<number> {
    const recorder = this.recorder;
    if (!recorder) return 0;
    this.recorder = null;
    await new Promise<void>((resolve) => {
      recorder.onstop = () => resolve();
      recorder.stop();
    });
    const video = new Uint8Array(await new Blob(this.chunks).arrayBuffer());
    const payload = frameRecording(
      {
        events: this.log.events,
        metadata: {
          width: this.canvas.width,
          height: this.canvas.height,
          duration: (performance.now() - this.started) / 1000,
          mime_type: recorder.mimeType,
        },
      },
      video,
    );
    await new Promise<void>((resolve, reject) => {
      const socket = new WebSocket(this.url);
      socket.binaryType = "arraybuffer";
      socket.onopen = () => {
        const step = 1024 * 1024;
        for (let at = 0; at < payload.byteLength; at += step) socket.send(payload.slice(at, at + step));
        socket.close(1000);
      };
      socket.onclose = () => resolve();
      socket.onerror = () => reject(new Error(`could not reach the recorder at ${this.url}`));
    });
    return payload.byteLength;
  }
}
