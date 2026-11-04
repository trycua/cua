// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A Space's Stream section and its picture-in-picture panels (the SwiftUI
// app's StreamRowsModel.swift and libs/spaces-app-swift's StreamPiPSet): the
// windows and the primary display the section's rows are built from, and
// one floating panel per source (the desktop, or one window). The rows
// themselves are the core's (`appStreamSection`, read by the page); this only
// fetches what they are built from and says which panels are open. No
// Electron here: the panels are drawn by whoever `PipPresenter` is (`pip.ts`).
import type { Native } from "../native/load";
import type { AppStreamDisplay } from "../native/generated/index";
import { withTimeout } from "./time";

/** A window in the Space that can be streamed (the SwiftUI app's `StreamWindow`). */
export interface StreamWindow {
  /** Opaque window handle from the Space's window list. Never a pid. */
  id: string;
  app: string;
  title: string;
  /** Target identity generation: a handle is only valid with its current epoch. */
  epoch: number;
  /** Surface size in pixels (0 when unknown). */
  width: number;
  height: number;
  appId: string;
  /** 0 when unknown. */
  pid: number;
}

/** What a stream view shows. */
export type StreamSource = { kind: "desktop" } | { kind: "window"; window: StreamWindow };

export const DESKTOP: StreamSource = { kind: "desktop" };

/** One panel per desktop or window handle, whatever its epoch or size (`StreamSource.pipKey`). */
export function pipKey(source: StreamSource): string {
  return source.kind === "desktop" ? "desktop" : `window:${source.window.id}`;
}

/** The panel's title: the window's name, or "Desktop" (`StreamSource.pipTitle`). */
export function pipTitle(source: StreamSource): string {
  if (source.kind === "desktop") return "Desktop";
  return source.window.title === "" ? source.window.app : source.window.title;
}

/** A window as the core's `AppRemoteWindow` (`StreamRowsModel.remote`), in the page's JSON. */
export function remoteWindow(w: StreamWindow) {
  const px = (v: number) => (v > 0 ? Math.round(v) : null);
  return {
    id: w.id,
    appName: w.app,
    title: w.title,
    visible: true,
    appId: w.appId,
    targetEpoch: w.epoch,
    widthPx: px(w.width),
    heightPx: px(w.height),
    pid: w.pid > 0 ? w.pid : null,
  };
}

/** A Space's windows and primary display (`StreamRowsModel`). */
export class StreamRows {
  /** How long a window listing may take (s): past it, the list reads as failed. */
  static windowsTimeout = 15;
  /** Null while the first list loads. */
  windows: StreamWindow[] | null = null;
  failed = false;
  display: AppStreamDisplay | null = null;

  constructor(
    private readonly native: Native,
    private readonly listWindows: () => Promise<StreamWindow[]>,
    private readonly primaryDisplay: () => Promise<AppStreamDisplay | null>,
  ) {}

  /** Reads the windows and the display once. A failed read keeps what was listed. */
  async refresh(): Promise<void> {
    const display = this.primaryDisplay().catch(() => null);
    const listed = await withTimeout(StreamRows.windowsTimeout, () => this.listWindows());
    if (listed.ok) {
      this.windows = listed.value;
      this.failed = false;
    } else {
      if (this.windows === null) this.windows = [];
      this.failed = true;
    }
    const d = await display;
    if (d) this.display = d;
  }

  /** The window a row stands for. */
  window(id: string): StreamWindow | undefined {
    return this.windows?.find((w) => w.id === id);
  }

  /** The rows whose panel is open, from the open panels' keys (the core's `synced` event). */
  openRows(openKeys: ReadonlySet<string>): string[] {
    const rows = [
      ...(openKeys.has(pipKey(DESKTOP)) ? [this.native.appStreamDesktopRowId()] : []),
      ...(this.windows ?? []).filter((w) => openKeys.has(pipKey({ kind: "window", window: w }))).map((w) => w.id),
    ];
    return this.native.appStreamPipReduce([], new this.native.AppPipEvent.Synced({ rows }));
  }
}

/** What a panel shows: a Space's desktop or one of its windows. */
export interface PipSpec {
  spaceId: string;
  spaceName: string;
  /** The Space's OS (a Mac's ⌘ goes to a Linux or Windows Space as Control). */
  os: string;
  key: string;
  title: string;
  source: StreamSource;
}

/** Draws the panels (`pip.ts`: floating windows). */
export interface PipPresenter {
  /** Shows the panel (or brings an open one forward); `closed` runs once when it goes, however it goes. */
  open(spec: PipSpec, closed: () => void): void;
  close(spaceId: string, key: string): void;
}

/**
 * A Space's open panels, one per source (`StreamPiPSet`). Each panel opens a
 * stream of its own and stops it when it closes; closing a panel by its own
 * close button is the same as popping it in.
 */
export class PipSet {
  readonly openKeys = new Set<string>();

  constructor(
    private readonly spaceId: string,
    /** The Space's name and OS, as listed now. */
    private readonly space: () => { name: string; os: string },
    private readonly presenter: PipPresenter,
  ) {}

  isOpen(source: StreamSource): boolean {
    return this.openKeys.has(pipKey(source));
  }

  popOut(source: StreamSource): void {
    const key = pipKey(source);
    this.openKeys.add(key);
    const { name, os } = this.space();
    this.presenter.open({ spaceId: this.spaceId, spaceName: name, os, key, title: pipTitle(source), source }, () => this.openKeys.delete(key));
  }

  popIn(source: StreamSource): void {
    const key = pipKey(source);
    if (!this.openKeys.has(key)) return;
    this.openKeys.delete(key);
    this.presenter.close(this.spaceId, key);
  }

  /** Closes every panel (the Space went away, or the app quits). */
  popInAll(): void {
    for (const key of [...this.openKeys]) {
      this.openKeys.delete(key);
      this.presenter.close(this.spaceId, key);
    }
  }
}
