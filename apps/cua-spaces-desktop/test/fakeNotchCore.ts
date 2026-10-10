// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// A scripted stand-in for the app core's notch functions, shaped like the
// cua-spaces-ffi Node bindings (numbers for flat enums, `{ tag, inner }`
// events and effects, bigint times). It does not reimplement the core: each
// event moves a few flags and asks for the effects the real reducer asks
// for, so the tests check the plumbing around the core (timers, effects,
// layout, window drags, thumbnails), not the core's rules.

import type { CoreNotchState, CoreTriggerState, NotchCore, Tagged } from "../src/notch/core";
import type { NotchLayout, ScreenFacts } from "../src/notch/protocol";

export interface FakeState extends CoreNotchState {
  hovering: boolean;
  permissionMissing: boolean;
  drag: { phase: number; windowId?: number; targetSpaceId?: string };
}

export interface FakeSpace {
  id: string;
  name: string;
  live?: boolean;
}

export const MOTION = {
  hoverDwellMs: 300,
  closeDelayMs: 400,
  openResponse: 0.42,
  openDamping: 0.8,
  closeResponse: 0.45,
  closeDamping: 1,
  reducedDuration: 0.2,
  hoverResponse: 0.26,
  hoverDamping: 0.65,
  hoverScale: 1.08,
  hoverScaleY: 1.12,
  contentDelayMs: 90,
  contentIn: 0.22,
  contentOut: 0.12,
  contentScale: 0.96,
};

export function layoutFor(screen: ScreenFacts, prompt: boolean): NotchLayout {
  const r = (x: number, y: number, width: number, height: number) => ({ x, y, width, height });
  const top = screen.frame.y + screen.frame.height;
  const open = prompt ? 236 : 200;
  return {
    hasNotch: screen.safeAreaTop > 0,
    notch: r(660, top - 32, 192, 32),
    closedFrame: r(640, top - 42, 232, 42),
    openFrame: r(436, top - open, 640, open),
    promptFrame: r(641, top - 60, 230, 60),
    tabFrame: r(852, top - 32, 44, 32),
    tabInsetNotch: 0,
    tabInsetOuter: 2,
    stageFrame: r(416, top - open - 60, 680, open + 60),
    notchStyle: screen.safeAreaTop > 0,
  };
}

export function fakeCore() {
  const events: Tagged[] = [];
  const triggerEvents: Tagged[] = [];
  const tileAtCalls: { row: boolean; x: number; y: number }[] = [];
  const estimates: [bigint, number][] = [];
  /** What the next trigger event returns (overlay events, tick). */
  let triggerScript: (e: Tagged) => { phase: number; kind: number; overlay: Tagged[]; tickAtMs?: bigint } = () => ({
    phase: 0,
    kind: 0,
    overlay: [],
  });

  const core: NotchCore = {
    appNotchInitial: (): FakeState => ({
      open: false,
      query: "",
      drag: { phase: 0 },
      hotspot: false,
      transfer: undefined,
      keyvault: undefined,
      signedIn: [],
      hidden: false,
      hovering: false,
      permissionMissing: false,
    }),
    appNotchReduce(state, event) {
      events.push(event);
      const s = { ...(state as FakeState) };
      const effects: Tagged[] = [];
      const inner = event.inner ?? {};
      if (s.hidden && ["HoverEnter", "DwellElapsed", "Click"].includes(event.tag)) return { state: s, effects };
      switch (event.tag) {
        case "HoverEnter":
          s.hovering = true;
          if (!s.open) effects.push({ tag: "StartDwell", inner: { ms: MOTION.hoverDwellMs } });
          else effects.push({ tag: "CancelTimers" });
          break;
        case "HoverExit":
          s.hovering = false;
          effects.push(s.open ? { tag: "StartClose", inner: { ms: MOTION.closeDelayMs } } : { tag: "CancelTimers" });
          break;
        case "DwellElapsed":
          s.open = true;
          break;
        case "CloseElapsed":
        case "Dismiss":
        case "Escape":
          s.open = false;
          s.query = "";
          break;
        case "Click":
          s.open = true;
          break;
        case "Search":
          s.query = inner.query as string;
          break;
        case "Visibility":
          s.hidden = !(inner.shown as boolean);
          if (s.hidden) s.open = false;
          break;
        case "Activity":
          s.hotspot = inner.hotspot as boolean;
          s.transfer = inner.transfer;
          break;
        case "Keyvault":
          s.keyvault = inner.label as string | undefined;
          s.signedIn = inner.signedIn as string[];
          break;
        case "DragPermission":
          s.permissionMissing = !(inner.granted as boolean);
          break;
        case "Drag": {
          const e = inner.event as Tagged;
          const d = { ...s.drag };
          if (e.tag === "Start") {
            d.phase = 1;
            d.windowId = e.inner?.windowId as number | undefined;
            effects.push({ tag: "Drag", inner: { effect: { tag: "Capture", inner: { windowId: d.windowId } } } });
          } else if (e.tag === "EnterNotch") d.phase = 2;
          else if (e.tag === "Over") d.targetSpaceId = e.inner?.spaceId as string;
          else if (e.tag === "Out") d.targetSpaceId = undefined;
          else if (e.tag === "Drop") {
            const id = e.inner?.spaceId as string | undefined;
            if (id) effects.push({ tag: "Drag", inner: { effect: { tag: "Commit", inner: { spaceId: id } } } });
            d.phase = 0;
          } else if (e.tag === "Cancel") d.phase = 0;
          s.drag = d;
          break;
        }
      }
      return { state: s, effects };
    },
    appNotchView(state, spaces) {
      const s = state as FakeState;
      const list = spaces as FakeSpace[];
      const phase = s.open || s.drag.phase === 2 ? 1 : s.drag.phase === 1 ? 2 : 0;
      return {
        phase,
        tiles: list.map((sp) => ({
          id: sp.id,
          name: sp.name,
          os: 0,
          status: sp.live === false ? 3 : 1,
          dim: sp.live === false,
          dropTarget: sp.live !== false,
          targeted: s.drag.targetSpaceId === sp.id,
          symbol: sp.id.startsWith("cloud:") ? "ubuntu" : "apple",
          label: sp.name,
          location: "This Mac",
          progress: undefined,
          progressLabel: undefined,
          signedIn: s.signedIn.includes(sp.id),
        })),
        dropMode: s.drag.phase === 2,
        prompt: s.drag.phase === 1 ? "Teleport to Cua" : undefined,
        label: "Cua Spaces",
        countLabel: `${list.length} Spaces`,
        tab: { count: String(list.length), word: "Spaces" },
        header: s.open ? { query: s.query, placeholder: "Search", searchLabel: "Search", matchCount: undefined, buttons: [] } : undefined,
        empty: undefined,
        activity: s.transfer
          ? { kind: 0, label: "Sending", symbol: undefined, permille: undefined, startedAt: undefined, estimateMs: 10_000 }
          : s.hotspot
            ? { kind: 2, label: "Hotspot", symbol: "personalhotspot", permille: undefined, startedAt: undefined, estimateMs: 0 }
            : undefined,
        hidden: s.hidden,
        showTab: !s.hidden && !s.open,
        hoverCue: s.hovering && !s.open,
        permission: s.open && s.permissionMissing ? { text: "Allow Accessibility", action: "Open Settings", pane: "accessibility" } : undefined,
        access: s.open && s.keyvault ? { text: s.keyvault, dismiss: "Dismiss" } : undefined,
      };
    },
    appNotchLayout: layoutFor,
    appNotchMotion: () => ({ ...MOTION }),
    appNotchRadii: () => [
      { top: 6, bottom: 14 },
      { top: 19, bottom: 24 },
    ],
    appNotchTileAt(_layout, tiles, row, x, y) {
      tileAtCalls.push({ row, x, y });
      return x < 600 ? (tiles[0] as { id: string } | undefined)?.id : undefined;
    },
    appNotchEstimatedProgress(elapsed, estimate) {
      estimates.push([elapsed, estimate]);
      return Math.min(950, Math.round((Number(elapsed) * 1000) / estimate));
    },
    appOsIconSystemSymbol: (id) => (id === "apple" ? "apple.logo" : undefined),
    appOsIconSvg: (id) => (id === "ubuntu" ? "<svg/>" : undefined),
    appDragTriggerInitial: (): CoreTriggerState => ({ kind: 0, phase: 0 }),
    appDragTriggerApply(_state, event) {
      triggerEvents.push(event);
      const r = triggerScript(event);
      return { state: { kind: r.kind, phase: r.phase }, overlay: r.overlay, tickAtMs: r.tickAtMs };
    },
    appDragDisplays: (screens) => screens.map((s) => ({ display: s })),
  };
  return {
    core,
    events,
    triggerEvents,
    tileAtCalls,
    estimates,
    script(f: typeof triggerScript) {
      triggerScript = f;
    },
  };
}

/** A clock the tests step by hand. */
export function manualClock(start = 1_000_000) {
  let now = start;
  let next = 0;
  let timers: { id: number; at: number; fire: () => void }[] = [];
  return {
    after(ms: number, fire: () => void) {
      const id = ++next;
      timers.push({ id, at: now + ms, fire });
      return () => {
        timers = timers.filter((t) => t.id !== id);
      };
    },
    monotonic: () => now,
    now: () => now,
    get pending() {
      return timers.length;
    },
    /** Advances, firing due timers in order (bounded). */
    advance(ms: number) {
      const end = now + ms;
      for (let i = 0; i < 1000; i++) {
        const due = timers.filter((t) => t.at <= end).sort((a, b) => a.at - b.at)[0];
        if (!due) break;
        timers = timers.filter((t) => t.id !== due.id);
        now = due.at;
        due.fire();
      }
      now = end;
    },
  };
}
