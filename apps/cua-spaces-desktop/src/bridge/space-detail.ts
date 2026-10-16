// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A Space's detail (`spaces.usage`, `spaces.windows`, `stream.pip`,
// `spaces.thumbnail`): its memory and storage, its Stream section's windows
// and display, its picture-in-picture panels and its latest thumbnail (the
// SwiftUI host's WebUIBridge+Pages.swift and +Thumbnails.swift).
import type { AppModel } from "../model/app-model";
import { DESKTOP, PipSet, remoteWindow, StreamRows, type PipPresenter, type StreamSource } from "../model/streams";
import { thumbnailDataUrl } from "../model/thumbnails";
import { object, string } from "./args";
import type { BridgeContext } from "./context";
import { Failure, type BridgeArgs, type Handlers } from "./host";
import { encode } from "./value";

/** A Space's stream rows and panels (null in a build that draws no panels). */
export interface SpaceStream {
  rows: StreamRows;
  pips: PipSet | null;
}

/**
 * Each Space's stream rows and panels, made once per Space on first use
 * (the SwiftUI host's `stream(for:)`) and dropped, panels closed, when the
 * Space leaves a loaded list (`pruneStreams`). Teleport's
 * `teleport.streamWindow` opens its panel through the same set.
 */
export class SpaceStreams {
  private readonly streams = new Map<string, SpaceStream>();
  private following = false;

  constructor(
    private readonly model: AppModel,
    private readonly presenter: PipPresenter | null,
  ) {}

  get(spaceId: string): SpaceStream {
    const held = this.streams.get(spaceId);
    if (held) return held;
    if (!this.following) {
      this.following = true;
      this.model.subscribe((change) => {
        if (change === "spaces") this.prune();
      });
    }
    const space = this.model.spaces.find((s) => s.id === spaceId);
    if (!space) throw Failure.notFound(`no Space ${spaceId}`);
    const backend = this.model.backend;
    const presenter = this.presenter;
    const s: SpaceStream = {
      rows: new StreamRows(
        this.model.native,
        () => backend.windows(spaceId),
        () => backend.primaryDisplay(spaceId),
      ),
      pips: presenter ? new PipSet(spaceId, () => this.spaceOf(spaceId), presenter) : null,
    };
    this.streams.set(spaceId, s);
    return s;
  }

  /** A listed Space's name and OS (the core's word: `linux`, `macos`, ...). */
  private spaceOf(spaceId: string): { name: string; os: string } {
    const s = this.model.spaces.find((x) => x.id === spaceId);
    return { name: s?.name ?? spaceId, os: s ? String(s.os) : "unknown" };
  }

  /** Closes and drops the streams of Spaces that are gone (a loaded list only: an empty one is a launch, not a deletion). */
  prune(): void {
    if (!this.model.loaded) return;
    const ids = new Set(this.model.spaces.map((s) => s.id));
    for (const [id, s] of this.streams) {
      if (ids.has(id)) continue;
      s.pips?.popInAll();
      this.streams.delete(id);
    }
  }

  /** Closes every panel (the app quits). */
  closeAll(): void {
    for (const s of this.streams.values()) s.pips?.popInAll();
  }
}

const shared = new WeakMap<BridgeContext, SpaceStreams>();

/** The bridge's stream rows and panels (one per bridge). */
export function spaceStreams(ctx: BridgeContext): SpaceStreams {
  let s = shared.get(ctx);
  if (!s) {
    s = new SpaceStreams(ctx.model, ctx.ui.pip ?? null);
    shared.set(ctx, s);
  }
  return s;
}

export function spaceDetailMethods(ctx: BridgeContext): Handlers {
  const { model } = ctx;
  const streams = spaceStreams(ctx);
  const spaceId = (args: BridgeArgs) => string(args, "spaceId");

  return {
    "spaces.usage": async (args) => encode(await model.backend.usage(spaceId(args))),
    "spaces.windows": async (args) => {
      const { rows, pips } = streams.get(spaceId(args));
      await rows.refresh();
      // The open panels too: one closed by its own button (or from the viewer) shows on the next read.
      return { windows: (rows.windows ?? []).map(remoteWindow), display: encode(rows.display), open: pips ? rows.openRows(pips.openKeys) : [] };
    },
    "stream.pip": async (args) => {
      const { rows, pips } = streams.get(spaceId(args));
      if (!pips) throw Failure.unsupported("Picture in picture is not available in this build");
      const command = object(args, "command");
      const type = command.type;
      const row = command.row;
      if (typeof type !== "string" || typeof row !== "string") throw Failure.badArgs("command: {type, row}");
      const desktopRow = model.native.appStreamDesktopRowId();
      if (row !== desktopRow && !rows.window(row)) await rows.refresh();
      let source: StreamSource;
      if (row === desktopRow) source = DESKTOP;
      else {
        const window = rows.window(row);
        if (!window) throw Failure.notFound(`no window ${row}`);
        source = { kind: "window", window };
      }
      if (type === "open") pips.popOut(source);
      else pips.popIn(source);
      return rows.openRows(pips.openKeys);
    },
    "spaces.thumbnail": async (args) => {
      const id = spaceId(args);
      if (!model.spaces.some((s) => s.id === id)) throw Failure.notFound(`no Space ${id}`);
      const maxAgeMs = typeof args.maxAgeMs === "number" && Number.isFinite(args.maxAgeMs) ? args.maxAgeMs : model.thumbnails.policy.backgroundIntervalMs;
      const entry = await model.thumbnails.refresh(id, maxAgeMs / 1000);
      return entry ? { url: thumbnailDataUrl(entry), capturedAtMs: entry.capturedAtMs } : null;
    },
  };
}
