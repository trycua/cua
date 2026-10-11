// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it, vi } from "vitest";
import { NotchModel, settingsURL, type NotchActions, type NotchTeleport, type WindowDragEvent } from "../src/notch/model";
import type { ScreenFacts, StateMessage } from "../src/notch/protocol";
import { fakeCore, manualClock, MOTION, type FakeSpace } from "./fakeNotchCore";

const SCREEN: ScreenFacts = {
  frame: { x: 0, y: 0, width: 1512, height: 982 },
  visibleFrame: { x: 0, y: 0, width: 1512, height: 945 },
  safeAreaTop: 32,
  auxLeftWidth: 662,
  auxRightWidth: 662,
};
const SPACES: FakeSpace[] = [
  { id: "local:aurora", name: "aurora" },
  { id: "cloud:build", name: "build", live: false },
];

function actions(): NotchActions & { calls: unknown[][] } {
  const calls: unknown[][] = [];
  const record =
    (name: string) =>
    (...args: unknown[]) =>
      void calls.push([name, ...args]);
  return {
    calls,
    openSpace: record("openSpace"),
    openMain: record("openMain"),
    openSettings: record("openSettings"),
    openAccess: record("openAccess"),
    dismissAccess: record("dismissAccess"),
    teleport: record("teleport"),
    drop: record("drop"),
  };
}

function setup(opts: { teleport?: NotchTeleport; thumbnail?: (id: string) => Promise<Uint8Array | null> } = {}) {
  const fake = fakeCore();
  const clock = manualClock();
  const states: StateMessage[] = [];
  const thumbnails: [string, Uint8Array | null][] = [];
  const ghosts: (Uint8Array | null)[] = [];
  const opened: string[] = [];
  const acts = actions();
  const model = new NotchModel(
    { core: fake.core, actions: acts, teleport: opts.teleport, thumbnail: opts.thumbnail },
    { state: (s) => states.push(s), thumbnail: (id, i) => thumbnails.push([id, i]), ghost: (g) => ghosts.push(g) },
    { clock, openURL: (u) => opened.push(u), log: () => {} },
  );
  model.setSpaces(SPACES);
  const last = () => states[states.length - 1]!;
  return { fake, clock, states, last, thumbnails, ghosts, opened, acts, model };
}

describe("notch model", () => {
  it("hands the helper the core's motion and radii", () => {
    const { model } = setup();
    expect(model.motion()).toEqual({ motion: MOTION, radii: { closed: { top: 6, bottom: 14 }, open: { top: 19, bottom: 24 } } });
  });

  it("sends the core's view, the tiles' icons and no layout until the helper reports its screens", () => {
    const { last, states, model } = setup();
    expect(last().view.tiles.map((t) => [t.id, t.status, t.dropTarget])).toEqual([
      ["local:aurora", "running", true],
      ["cloud:build", "suspended", false],
    ]);
    expect(last().icons).toEqual({ apple: { symbol: "apple.logo" }, ubuntu: { svg: "<svg/>" } });
    expect(last().layout).toBeUndefined();
    expect(last().shown).toBe(true);
    // Unchanged state is not sent again.
    const n = states.length;
    model.setSpaces([...SPACES]);
    expect(states.length).toBe(n);
  });

  it("lays out for the primary screen when no screen has a notch, so the helper gets a layout", () => {
    // A Mac without a notch, where the helper finds no main screen either
    // (an agent app with no key window): the state still carries a layout.
    const { model, last } = setup();
    const plain: ScreenFacts = { ...SCREEN, safeAreaTop: 0, auxLeftWidth: undefined, auxRightWidth: undefined };
    model.screens(undefined, plain);
    expect(last().layout).toBeDefined();
    expect(last().shown).toBe(true);
    // No screen at all: no layout, and the notch still shows (the helper keeps its panel).
    const bare = setup();
    bare.model.screens(undefined, undefined);
    expect(bare.last().layout).toBeUndefined();
    expect(bare.last().shown).toBe(true);
  });

  it("lays out for the notch screen and re-lays out when the row above the tiles comes", () => {
    const { model, last, fake } = setup();
    model.screens(SCREEN, SCREEN);
    expect(last().layout?.openFrame.height).toBe(200);
    // A window drag puts the prompt line up: the layout gets the row.
    model.send({ tag: "Drag", inner: { event: { tag: "Start", inner: { windowId: 7, appName: "Notes" } } } });
    expect(last().view.prompt).toBe("Teleport to Cua");
    expect(last().layout?.openFrame.height).toBe(236);
    expect(last().dragging).toBe(true);
    expect(fake.events.at(-1)?.tag).toBe("Drag");
  });

  it("opens after the core's dwell, not a millisecond early, and closes after its delay", () => {
    const { model, clock, last } = setup();
    model.input({ kind: "hoverEnter" });
    expect(last().view.hoverCue).toBe(true);
    clock.advance(MOTION.hoverDwellMs - 1);
    expect(last().view.phase).toBe("closed");
    clock.advance(1);
    expect(last().view.phase).toBe("tiles");
    model.input({ kind: "hoverExit" });
    clock.advance(MOTION.closeDelayMs - 1);
    expect(last().view.phase).toBe("tiles");
    clock.advance(1);
    expect(last().view.phase).toBe("closed");
  });

  it("cancels the dwell when the pointer leaves early", () => {
    const { model, clock, last } = setup();
    model.input({ kind: "hoverEnter" });
    clock.advance(100);
    model.input({ kind: "hoverExit" });
    expect(clock.pending).toBe(0);
    clock.advance(1000);
    expect(last().view.phase).toBe("closed");
  });

  it("passes clicks, Escape, file drags and the search text to the core", () => {
    const { model, fake, last } = setup();
    model.input({ kind: "click" });
    model.input({ kind: "search", query: "au" });
    model.input({ kind: "search", query: "au" });
    expect(last().query).toBe("au");
    model.input({ kind: "dropTargeted", targeted: true });
    model.input({ kind: "escape" });
    model.input({ kind: "dismiss" });
    expect(fake.events.map((e) => e.tag)).toEqual(["Click", "Search", "DropTargeted", "Escape", "Dismiss"]);
    expect(fake.events[2]?.inner).toEqual({ targeted: true });
  });

  it("feeds the activity and the Keyvault sign-ins, only when they change", () => {
    const { model, fake, last } = setup();
    model.setActivity(false, null);
    model.setKeyvault(null, []);
    expect(fake.events).toEqual([]);
    model.setActivity(true);
    model.setActivity(true);
    expect(fake.events.map((e) => e.tag)).toEqual(["Activity"]);
    expect(last().view.activity?.symbol).toBe("personalhotspot");
    model.setActivity(false, { sent: 600, total: 1000 });
    // u64 fields go to the core as bigint.
    expect(fake.events.at(-1)?.inner).toEqual({ hotspot: false, transfer: { sent: 600n, total: 1000n } });
    model.setActivity(false, { sent: 600n, total: 1000n });
    expect(fake.events.filter((e) => e.tag === "Activity")).toHaveLength(2);
    model.setKeyvault("Sign-ins live in aurora", ["local:aurora"]);
    model.setKeyvault("Sign-ins live in aurora", ["local:aurora"]);
    expect(fake.events.filter((e) => e.tag === "Keyvault")).toHaveLength(1);
    expect(last().view.tiles[0]?.signedIn).toBe(true);
  });

  it("estimates a ring's progress with the core while it shows, every 200 ms", () => {
    const { model, clock, last, fake } = setup();
    model.setActivity(false, { sent: undefined, total: undefined });
    expect(last().view.activity?.permille).toBe(0);
    clock.advance(1000);
    expect(last().view.activity?.permille).toBe(100);
    expect(fake.estimates.at(-1)).toEqual([1000n, 10_000]);
    model.setActivity(false, null);
    expect(last().view.activity).toBeUndefined();
    expect(clock.pending).toBe(0);
  });

  it("hides and shows with the setting: hover and clicks do nothing while hidden", () => {
    const { model, last, fake } = setup();
    model.setShown(false);
    expect(model.isShown).toBe(false);
    expect(last().shown).toBe(false);
    expect(last().view.hidden).toBe(true);
    expect(fake.events.at(-1)).toEqual({ tag: "Visibility", inner: { shown: false } });
    model.input({ kind: "click" });
    expect(last().view.phase).toBe("closed");
    model.setShown(true);
    expect(last().shown).toBe(true);
    expect(fake.events.at(-1)).toEqual({ tag: "Visibility", inner: { shown: true } });
    model.input({ kind: "click" });
    expect(last().view.phase).toBe("tiles");
  });

  it("refreshes live tiles' thumbnails while open, every 3 s, and stops when it closes", async () => {
    const fetched: string[] = [];
    const image = new Uint8Array([0x89, 0x50]);
    const { model, clock, thumbnails } = setup({
      thumbnail: async (id) => {
        fetched.push(id);
        return image;
      },
    });
    model.stageChanged(true);
    await vi.waitFor(() => expect(thumbnails).toEqual([["local:aurora", image]]));
    expect(fetched).toEqual(["local:aurora"]);
    clock.advance(3000);
    await vi.waitFor(() => expect(fetched).toEqual(["local:aurora", "local:aurora"]));
    model.stageChanged(false);
    clock.advance(30_000);
    await Promise.resolve();
    expect(fetched).toHaveLength(2);
  });

  it("warms the thumbnails on the hover cue, at most every 3 s", async () => {
    const fetched: string[] = [];
    const { model, clock } = setup({ thumbnail: async (id) => (fetched.push(id), null) });
    model.input({ kind: "hoverEnter" });
    await vi.waitFor(() => expect(fetched).toHaveLength(1));
    model.input({ kind: "hoverExit" });
    model.input({ kind: "hoverEnter" });
    await Promise.resolve();
    expect(fetched).toHaveLength(1);
    model.input({ kind: "hoverExit" });
    clock.advance(3001);
    model.input({ kind: "hoverEnter" });
    await vi.waitFor(() => expect(fetched).toHaveLength(2));
  });

  it("opens the permission's System Settings pane after registering for it", () => {
    const teleport = fakeTeleport({ permitted: false });
    const { model, opened } = setup({ teleport });
    model.openPermissionSettings("accessibility");
    expect(teleport.requested).toBe(1);
    expect(opened).toEqual([settingsURL("accessibility")]);
    expect(settingsURL("other")).toBe("x-apple.systempreferences:com.apple.preference.security");
  });
});

function fakeTeleport(o: { permitted: boolean; supported?: boolean }) {
  const t = {
    permitted: o.permitted,
    requested: 0,
    started: 0,
    stopped: 0,
    listener: undefined as undefined | { onEvent(e: WindowDragEvent): void },
    windowDragSupported: () => o.supported ?? true,
    windowDragPermitted: () => t.permitted,
    requestWindowDragPermission: () => (t.requested++, true),
    startWindowDrag(listener: { onEvent(e: WindowDragEvent): void }) {
      t.started++;
      t.listener = listener;
      return { stop: () => void t.stopped++ };
    },
    captureWindowThumbnail: (id: number) => (id === 7 ? new Uint8Array([1, 2, 3]) : null),
  };
  return t;
}

describe("notch window drags", () => {
  it("shows the permission line until it is granted, checking every 2 s, then follows drags", async () => {
    const teleport = fakeTeleport({ permitted: false });
    const { model, clock, fake } = setup({ teleport });
    model.recheckPermission();
    expect(fake.events.at(-1)).toEqual({ tag: "DragPermission", inner: { granted: false } });
    expect(teleport.started).toBe(0);
    clock.advance(2000);
    expect(teleport.started).toBe(0);
    teleport.permitted = true;
    clock.advance(2000);
    expect(fake.events.at(-1)).toEqual({ tag: "DragPermission", inner: { granted: true } });
    await vi.waitFor(() => expect(teleport.started).toBe(1));
    expect(clock.pending).toBe(0);
    model.recheckPermission();
    expect(teleport.started).toBe(1);
    // Hidden: the monitor stops and starts again when shown.
    model.setShown(false);
    expect(teleport.stopped).toBe(1);
    model.setShown(true);
    await vi.waitFor(() => expect(teleport.started).toBe(2));
  });

  it("does nothing without window-drag support", () => {
    const teleport = fakeTeleport({ permitted: true, supported: false });
    const { model, fake } = setup({ teleport });
    model.recheckPermission();
    expect(fake.events.filter((e) => e.tag === "DragPermission")).toEqual([]);
  });

  it("feeds drags to the core's trigger, hit-tests tiles and commits to the tile under the drop", async () => {
    const teleport = fakeTeleport({ permitted: true });
    const { model, fake, acts, ghosts, clock } = setup({ teleport });
    model.screens(SCREEN, SCREEN);
    model.recheckPermission();
    await vi.waitFor(() => expect(teleport.listener).toBeDefined());
    // The trigger's answers: the box at start, expanded (tiles) on the move,
    // a dwell tick, then the drop.
    fake.script((e) => {
      switch (e.tag) {
        case "Start":
          return { phase: 1, kind: 1, overlay: [{ tag: "Start", inner: { windowId: 7, appName: "Notes" } }] };
        case "Cursor":
          return { phase: 2, kind: 1, overlay: [{ tag: "EnterNotch" }], tickAtMs: BigInt(clock.monotonic() + 150) };
        case "End":
          return { phase: 0, kind: 1, overlay: [{ tag: "Drop" }] };
        default:
          return { phase: 2, kind: 1, overlay: [] };
      }
    });
    const app = { name: "Notes", id: "com.apple.Notes" };
    const frame = { x: 400, y: 300, width: 800, height: 600 };
    teleport.listener!.onEvent({ phase: "start", x: 700, y: 500, window: { windowId: 7, appName: "Notes" }, app, startFrame: frame, frame });
    expect(fake.triggerEvents[0]).toEqual({
      tag: "Start",
      inner: { windowId: 7, appName: "Notes", x: 700, y: 500, tMs: BigInt(clock.monotonic()), startFrame: frame, frame },
    });
    // The capture effect: the ghost goes to the helper, the core hears it is ready.
    expect(ghosts).toEqual([new Uint8Array([1, 2, 3])]);
    expect(fake.events.some((e) => (e.inner?.event as { tag?: string })?.tag === "GhostReady")).toBe(true);
    expect(model.dragKind).toBe("move");

    teleport.listener!.onEvent({ phase: "move", x: 500, y: 40, frame });
    // A move with a frame feeds the frame first.
    expect(fake.triggerEvents.slice(1).map((e) => e.tag)).toEqual(["Frame", "Cursor"]);
    // Expanded: the tile under the cursor (y flipped from the primary's top).
    expect(fake.tileAtCalls.at(-1)).toEqual({ row: false, x: 500, y: 982 - 40 });
    expect(fake.events.at(-1)?.inner?.event).toEqual({ tag: "Over", inner: { spaceId: "local:aurora" } });
    clock.advance(150);
    expect(fake.triggerEvents.at(-1)).toEqual({ tag: "Tick", inner: { tMs: BigInt(clock.monotonic()) } });

    teleport.listener!.onEvent({ phase: "end", x: 500, y: 40 });
    expect(acts.calls).toEqual([["teleport", "local:aurora", app]]);
    expect(ghosts.at(-1)).toBeNull();
  });

  it("drops window drags while hidden", async () => {
    const teleport = fakeTeleport({ permitted: true });
    const { model, fake } = setup({ teleport });
    model.recheckPermission();
    await vi.waitFor(() => expect(teleport.listener).toBeDefined());
    const listener = teleport.listener!;
    model.setShown(false);
    listener.onEvent({ phase: "start", x: 1, y: 1 });
    expect(fake.triggerEvents).toEqual([]);
  });
});
