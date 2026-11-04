// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A Space's viewer window ("Open in window"; viewer.ts, viewer-layout.ts):
// the SwiftUI app's `SpaceWindowView`, one window per Space titled with its
// name and showing only the web UI's viewer, shaped to the stream, where
// the last one was left; and the video bench, which places its windows
// itself and never saves them over the person's.
import { beforeEach, describe, expect, it, vi } from "vitest";

type Listener = (...args: unknown[]) => void;

/** A BrowserWindow with what the shell uses of it. */
class FakeWindow {
  static all: FakeWindow[] = [];
  readonly listeners = new Map<string, Listener[]>();
  readonly contentsListeners = new Map<string, Listener[]>();
  destroyed = false;
  shown = false;
  focused = 0;
  url = "";
  bounds: { x: number; y: number; width: number; height: number };
  content: [number, number];
  readonly webContents = {
    setWindowOpenHandler: () => {},
    on: (e: string, f: Listener) => this.contentsListeners.set(e, [...(this.contentsListeners.get(e) ?? []), f]),
    once: (e: string, f: Listener) => this.contentsListeners.set(e, [...(this.contentsListeners.get(e) ?? []), f]),
    insertCSS: async () => "k",
    removeInsertedCSS: async () => {},
  };
  constructor(readonly options: Record<string, unknown>) {
    const o = options as { x?: number; y?: number; width: number; height: number };
    this.bounds = { x: o.x ?? 0, y: o.y ?? 0, width: o.width, height: o.height + 28 };
    this.content = [o.width, o.height];
    FakeWindow.all.push(this);
  }
  on(e: string, f: Listener) {
    this.listeners.set(e, [...(this.listeners.get(e) ?? []), f]);
    return this;
  }
  once(e: string, f: Listener) {
    return this.on(e, f);
  }
  emit(e: string, ...args: unknown[]) {
    for (const f of this.listeners.get(e) ?? []) f(...args);
  }
  contentsEmit(e: string, ...args: unknown[]) {
    for (const f of this.contentsListeners.get(e) ?? []) f(...args);
  }
  loadURL(url: string) {
    this.url = url;
    return Promise.resolve();
  }
  show() {
    this.shown = true;
  }
  focus() {
    this.focused++;
  }
  restore() {}
  close() {
    this.emit("close");
    this.destroyed = true;
    this.emit("closed");
  }
  isDestroyed = () => this.destroyed;
  isMinimized = () => false;
  isMaximized = () => false;
  isFullScreen = () => false;
  getBounds = () => ({ ...this.bounds });
  getNormalBounds = () => ({ ...this.bounds });
  setBounds(b: { x: number; y: number; width: number; height: number }) {
    this.bounds = b;
    this.emit("move");
    this.emit("resize");
  }
  getContentSize = () => [...this.content];
  setContentSize(w: number, h: number) {
    this.content = [w, h];
  }
  maximize() {}
}

const work = { x: 0, y: 25, width: 1440, height: 875 };
const settings: { current: Record<string, unknown> } = { current: {} };

vi.mock("electron", () => ({
  app: { isPackaged: false, getPath: () => "/nowhere", quit: () => {} },
  BrowserWindow: FakeWindow,
  ipcMain: { on: () => {} },
  shell: { openExternal: () => {} },
  nativeTheme: { shouldUseDarkColors: true, themeSource: "system" },
  screen: {
    getPrimaryDisplay: () => ({ workArea: work }),
    getAllDisplays: () => [{ workArea: work }],
    getDisplayMatching: () => ({ workArea: work }),
  },
  protocol: {},
  net: {},
}));
vi.mock("../src/settings", () => ({
  readSettings: () => settings.current,
  writeSettings: (patch: Record<string, unknown>) => {
    settings.current = { ...settings.current, ...patch };
  },
}));

const { createViewerWindows } = await import("../src/viewer");
const { createMainWindow } = await import("../src/window");
const { fitViewerToStream, initialViewerBounds, viewerPath, VIEWER_CASCADE, VIEWER_MIN_WIDTH } = await import("../src/viewer-layout");

beforeEach(() => {
  FakeWindow.all = [];
  settings.current = {};
  vi.useRealTimers();
});

describe("the viewer's place and shape", () => {
  it("opens 960 wide at 16:10 under its toolbar, centred, until a place was saved", () => {
    expect(initialViewerBounds({ saved: null, work, areas: [work], open: 0 })).toEqual({ x: 240, y: 141, width: 960, height: 644 });
  });

  it("opens where the last one was left at its width, cascading beside an open one, and forgets a place off every display", () => {
    const saved = { x: 100, y: 120, width: 800 };
    expect(initialViewerBounds({ saved, work, areas: [work], open: 0 })).toEqual({ x: 100, y: 120, width: 800, height: 544 });
    expect(initialViewerBounds({ saved, work, areas: [work], open: 1 })).toMatchObject({ x: 100 + VIEWER_CASCADE, y: 120 + VIEWER_CASCADE });
    expect(initialViewerBounds({ saved: { x: 5000, y: 120, width: 800 }, work, areas: [work], open: 0 })).toMatchObject({ x: 320, width: 800 });
    expect(initialViewerBounds({ saved: { x: 0, y: 30, width: 200 }, work, areas: [work], open: 0 }).width).toBe(VIEWER_MIN_WIDTH);
  });

  it("takes the stream's shape at its width, narrower when that would not fit, never under the minimum", () => {
    expect(fitViewerToStream({ width: 960, height: 644 }, { width: 1280, height: 800 }, work)).toEqual({ width: 960, height: 644 });
    expect(fitViewerToStream({ width: 960, height: 644 }, { width: 1920, height: 1080 }, work)).toEqual({ width: 960, height: 584 });
    // 2560x1600 at 1400 wide would be 919 tall: narrowed to fit 90% of the work area (787).
    expect(fitViewerToStream({ width: 1400, height: 919 }, { width: 2560, height: 1600 }, work)).toEqual({ width: 1188, height: 787 });
    // A portrait window at the minimum width: letterboxed, no taller than the work area allows.
    expect(fitViewerToStream({ width: 960, height: 644 }, { width: 800, height: 1200 }, work)).toEqual({ width: 640, height: 787 });
    expect(fitViewerToStream({ width: 960, height: 644 }, { width: 0, height: 0 }, work)).toEqual({ width: 960, height: 644 });
  });

  it("loads the web UI's viewer for the Space", () => {
    expect(viewerPath("local:a b", "Aurora", "linux")).toBe("/viewer?space=local%3Aa+b&name=Aurora&os=linux");
    expect(viewerPath("cloud:x", "X")).toBe("/viewer?space=cloud%3Ax&name=X");
  });
});

describe("the viewer windows", () => {
  it("opens one window per Space, titled with its name, on the viewer, with the system title bar", () => {
    const viewers = createViewerWindows();
    const win = viewers.open("local:a", "Aurora", "linux") as unknown as FakeWindow;
    expect(win.url).toBe("cua-spaces://app/viewer?space=local%3Aa&name=Aurora&os=linux");
    expect(win.options).toMatchObject({ title: "Aurora", useContentSize: true, titleBarStyle: "default", titleBarOverlay: false, minWidth: 640, minHeight: 400, width: 960, height: 644 });
    win.emit("ready-to-show");
    expect(win.shown).toBe(true);
    // Open again: the same window, brought forward.
    expect(viewers.open("local:a", "Aurora", "linux")).toBe(win);
    expect(win.focused).toBe(1);
    const other = viewers.open("local:b", "Builder") as unknown as FakeWindow;
    expect(other).not.toBe(win);
    expect(other.options).toMatchObject({ x: 240 + VIEWER_CASCADE });
  });

  it("takes the stream's shape when the page says its size", () => {
    const win = createViewerWindows().open("local:a", "Aurora") as unknown as FakeWindow;
    const event = { preventDefault: vi.fn() };
    win.contentsEmit("content-bounds-updated", event, { x: 0, y: 0, width: 1920, height: 1080 });
    expect(event.preventDefault).toHaveBeenCalled();
    expect(win.getContentSize()).toEqual([960, 584]);
  });

  it("remembers where it was left, and the next one opens there", async () => {
    vi.useFakeTimers();
    const viewers = createViewerWindows();
    const win = viewers.open("local:a", "Aurora") as unknown as FakeWindow;
    win.setBounds({ x: 300, y: 200, width: 800, height: 572 });
    win.setContentSize(800, 544);
    await vi.advanceTimersByTimeAsync(500);
    expect(settings.current.viewerBounds).toEqual({ x: 300, y: 200, width: 800 });
    win.close();
    const next = viewers.open("local:a", "Aurora") as unknown as FakeWindow;
    expect(next).not.toBe(win);
    expect(next.options).toMatchObject({ x: 300, y: 200, width: 800, height: 544 });
  });

  it("closes the viewer of a Space that is no longer listed", () => {
    const viewers = createViewerWindows();
    const a = viewers.open("local:a", "Aurora") as unknown as FakeWindow;
    const b = viewers.open("local:b", "Builder") as unknown as FakeWindow;
    viewers.prune(new Set(["local:b"]));
    expect([a.destroyed, b.destroyed]).toEqual([true, false]);
  });
});

describe("the video bench's windows", () => {
  it("neither use nor overwrite the person's saved places", async () => {
    vi.useFakeTimers();
    settings.current = { windowBounds: { x: 10, y: 40, width: 1100, height: 700 }, viewerBounds: { x: 300, y: 200, width: 800 } };
    const before = structuredClone(settings.current);
    const main = createMainWindow({ remember: false }) as unknown as FakeWindow;
    expect(main.options).toMatchObject({ width: 1280, height: 820 });
    expect(main.options.x).toBeUndefined();
    const viewer = createViewerWindows({ remember: false }).open("local:a", "Aurora") as unknown as FakeWindow;
    expect(viewer.options).toMatchObject({ x: 240, width: 960 });
    // The bench's placement (startVideoBench), then a close.
    main.setBounds({ x: 40, y: 80, width: 1240, height: 860 });
    viewer.setBounds({ x: 1300, y: 80, width: 900, height: 700 });
    await vi.advanceTimersByTimeAsync(500);
    main.close();
    viewer.close();
    expect(settings.current).toEqual(before);
  });

  it("the main window still saves its place outside the bench", async () => {
    vi.useFakeTimers();
    const main = createMainWindow() as unknown as FakeWindow;
    main.setBounds({ x: 40, y: 80, width: 1240, height: 860 });
    await vi.advanceTimersByTimeAsync(500);
    expect(settings.current.windowBounds).toMatchObject({ x: 40, y: 80, width: 1240, height: 860, maximized: false });
  });
});
