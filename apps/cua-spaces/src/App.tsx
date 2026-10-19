// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useMemo, useReducer, useRef, useState } from "react";

import { Ambient } from "./components/Ambient";
import { AppTeleportPicker } from "./components/AppTeleportPicker";
import { createHostBridge, type HostBridge, type HostStatus } from "./native/host";
import { createSpacesListBridge, type SpacesListBridge } from "./native/spacesList";
import { NotchCap } from "./components/NotchCap";
import { Switcher } from "./components/Switcher";
import {
  applyDragOverlay,
  initialDragOverlay,
  type DragOverlayEffect,
  type DragOverlayEvent,
  type DragOverlayState,
} from "./model/dragOverlay";
import { entryFromCore, type CatalogEntry } from "@trycua/cua/teleport";

import type { LocalApp } from "./model/teleport";
import { handleAppDrop, hasAppPath } from "./native/appDrop";
import { createTeleportAppsBridge, type TeleportAppsBridge } from "./native/teleportApps";
import type { FleetDraft, Space } from "./model/types";
import type { NativeBridge } from "./native/bridge";
import { createFleetBridge, type FleetBridge, type HotspotStatus } from "./native/fleet";
import { createTeleportBridge, type TeleportBridge } from "./native/teleport";
import {
  createWindowDragBridge,
  screenToClient,
  type WindowDragBridge,
  type WindowDragEvent,
} from "./native/windowDrag";
import {
  applyDragTrigger,
  displayFor,
  fallbackDragDisplays,
  initialDragTrigger,
  type DragDisplay,
  type DragTriggerEvent,
} from "./model/dragTrigger";
import type { DisplayStyle, PortalEnvironment, WindowMode } from "./native/types";
import { agentsToolCall, type ToolCall } from "./native/persistent";
import { createSetTrayMenu, type SetTrayMenu } from "./native/tray";
import { FleetContext, useFleetSync, viewerRequestFor } from "./state/cloud";
import { useTrayMenu } from "./state/trayMenu";
import { NEW_SPACE_DROP_ID, NEW_TILE_INDEX, initialState, reduce, type PortalState } from "./state/portal";
import { readMenuBar, type SwitcherTheme } from "./state/settings";

export interface AppProps {
  bridge: NativeBridge;
  /** The shell's Spaces bridge; defaults to the environment-appropriate one. */
  fleet?: FleetBridge;
  /** App-teleport bridge; defaults to the environment-appropriate one. */
  teleport?: TeleportBridge;
  /** Opens the main window (Spaces list, New Space, Settings). */
  spacesList?: SpacesListBridge;
  /** AX window-drag bridge; defaults to the environment-appropriate one. */
  windowDrag?: WindowDragBridge;
  /** "Teleport an app…" over the SDK; defaults to the environment-appropriate one. */
  teleportApps?: TeleportAppsBridge;
  /** "This machine" host setup + first-run onboarding bridge. */
  host?: HostBridge;
  /** The menu bar item's menu: Cua Volume's sync read and the shell's
   * `tray_set_menu` (tests pass fakes). */
  tray?: { call: ToolCall; setMenu: SetTrayMenu };
  /** Clock injected for deterministic tests. */
  now?: () => number;
  /** Delay before the shelf collapses after a switch confirmation. */
  collapseDelayMs?: number;
  /** Optional preset state (tests). */
  initial?: PortalState;
}

const SWITCH_COLLAPSE_MS = 700;
const NOTICE_MS = 2600;
// How long the Island panel spends springing back into the notch before the
// window actually shrinks to the ambient tab. Matches --dur-island.
const ISLAND_COLLAPSE_MS = 200;

// Slightly longer than --dur-spring so the box's collapse animation finishes
// before the portal window shrinks back to the ambient tab.
const TELEPORT_SHRINK_MS = 340;

function reportBridgeError(operation: string, error: unknown) {
  console.error(`[Cua Spaces] ${operation} failed`, error);
}

export function App({ bridge, fleet, teleport, spacesList, windowDrag, teleportApps, host, tray, now = Date.now, collapseDelayMs = SWITCH_COLLAPSE_MS, initial }: AppProps) {
  const [state, dispatch] = useReducer(reduce, initial, (preset) => preset ?? initialState());
  const [env, setEnv] = useState<PortalEnvironment | null>(null);
  // Menu-bar presentation: when on, the ambient "N Spaces" tab is hidden and a
  // macOS menu-bar status item is the switcher entry point instead. Distinct
  // from `displayStyle` (notch/no-notch layout); persisted by state/settings.
  const [menuBar, setMenuBarState] = useState<boolean>(() => readMenuBar());
  // "Spaces tab in the notch: Hide" hides the whole notch UI, as the core's
  // notch state does (`hidden`): no tab, no hover cue, no drag teleport box.
  const menuBarRef = useRef(menuBar);
  menuBarRef.current = menuBar;
  // The notch panel is always the opaque black "island" that grows out of the
  // notch; the translucent theme is gone (it was unstable and broke captures).
  const theme: SwitcherTheme = "island";
  // True while the Island panel is playing its spring-back-into-the-notch exit
  // (the window is kept at switcher size until it finishes, then collapses).
  const [islandCollapsing, setIslandCollapsing] = useState(false);
  const islandCollapseTimer = useRef<number | null>(null);
  // Whether this Mac is sharing its network to a Space (drives the left-of-notch
  // hotspot indicator). Sourced from the native shell + its change events.
  const [hotspot, setHotspot] = useState<HotspotStatus>({ active: false, spaceId: null });
  // Cursor-over-notch, driven by the shell's global-cursor poller (CSS :hover
  // can't fire while the app is inactive). Springs the notch up a touch.
  const [notchHover, setNotchHover] = useState(false);
  // A file/session transfer in flight (teleport upload etc.); surfaced as a
  // progress indicator in the notch's left slot. Sourced from the shell.
  const [transfer, setTransfer] = useState<{ active: boolean; sent?: number; total?: number }>({
    active: false,
  });
  const collapseTimer = useRef<number | null>(null);
  const noticeTimer = useRef<number | null>(null);

  const defaultFleet = useMemo(() => createFleetBridge(), []);
  const fleetBridge = fleet ?? defaultFleet;
  const defaultSpacesList = useMemo(() => createSpacesListBridge(), []);
  const spacesListBridge = spacesList ?? defaultSpacesList;
  const defaultTeleport = useMemo(() => createTeleportBridge(), []);
  const teleportBridge = teleport ?? defaultTeleport;
  const defaultTeleportApps = useMemo(() => createTeleportAppsBridge(), []);
  const teleportAppsBridge = teleportApps ?? defaultTeleportApps;
  const defaultWindowDrag = useMemo(() => createWindowDragBridge(), []);
  const windowDragBridge = windowDrag ?? defaultWindowDrag;
  const fleetSync = useFleetSync(fleetBridge, dispatch, now);
  const defaultHost = useMemo(() => createHostBridge(), []);
  const hostBridge = host ?? defaultHost;
  // First-run onboarding lives in the main window; the notch only needs the
  // host status for its "This machine" entry.
  const [hostStatus, setHostStatus] = useState<HostStatus | null>(null);
  useEffect(() => {
    if (!hostBridge.isNative) return;
    let cancelled = false;
    void hostBridge
      .status()
      .then((next) => {
        if (!cancelled) setHostStatus(next);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [hostBridge]);
  const { setThisMachine } = fleetSync;
  useEffect(() => {
    setThisMachine(hostStatus);
  }, [hostStatus, setThisMachine]);
  // The menu bar item's menu: the core's, for this same roster (the notch
  // tab's count) and Cua Volume's sync.
  const defaultTray = useMemo(() => ({ call: agentsToolCall(), setMenu: createSetTrayMenu() }), []);
  useTrayMenu(state.spaces, tray ?? defaultTray);
  const fleetContext = useMemo(
    () => ({ live: fleetSync.live, bridge: fleetBridge }),
    [fleetSync.live, fleetBridge],
  );

  // Track the network hotspot: seed from the shell, then follow its change
  // events (so the indicator also clears if the tunnel drops on its own).
  useEffect(() => {
    if (!fleetBridge.isNative) return;
    let cancelled = false;
    let unsub: (() => void) | undefined;
    void fleetBridge
      .hotspotStatus()
      .then((status) => {
        if (!cancelled) setHotspot(status);
      })
      .catch(() => {});
    void fleetBridge.onHotspotChanged(setHotspot).then((off) => {
      if (cancelled) off();
      else unsub = off;
    });
    return () => {
      cancelled = true;
      unsub?.();
    };
  }, [fleetBridge]);
  const stopHotspot = useCallback(() => {
    void fleetBridge
      .stopHotspot()
      .then(setHotspot)
      .catch((error) => reportBridgeError("stop network sharing", error));
  }, [fleetBridge]);

  // Follow the shell's notch-hover signal (global cursor, focus-independent).
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void import("@tauri-apps/api/event")
      .then(({ listen }) => listen<boolean>("notch:hover", (event) => setNotchHover(!!event.payload)))
      .then((stop) => {
        if (cancelled) stop();
        else unlisten = stop;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
  // Captures: the shell can expand the panel without the cursor.
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void import("@tauri-apps/api/event")
      .then(({ listen }) => listen("portal:expand", () => dispatch({ type: "expand" })))
      .then((stop) => {
        if (cancelled) stop();
        else unlisten = stop;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
  // Follow the shell's transfer signal so an in-flight upload shows in the notch.
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void import("@tauri-apps/api/event")
      .then(({ listen }) =>
        listen<{ active: boolean; sent_bytes?: number; total_bytes?: number }>(
          "notch:transfer",
          (event) =>
            setTransfer({
              active: !!event.payload.active,
              sent: event.payload.sent_bytes,
              total: event.payload.total_bytes,
            }),
        ),
      )
      .then((stop) => {
        if (cancelled) stop();
        else unlisten = stop;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);
  /** The in-portal "Teleport an app…" sheet (browser preview only; the
   * native shell opens the picker window instead); null when closed. */
  const [teleportState, setTeleportState] = useState<{
    spaceId: string;
    spaceName: string;
    preselect: { entry: CatalogEntry; files?: string[] } | null;
  } | null>(null);
  /** True while an app window is being dragged over the switcher (drop mode). */
  const [dragActive, setDragActive] = useState(false);
  /** Display name of the dragged app, for the drop-mode header hint. */
  const [dragAppName, setDragAppName] = useState<string | null>(null);
  /** Captured preview (data URL) of the dragged window — the tiny ghost echo,
   * shown in the notch bulge and carried into the expanded selector. */
  const [dragGhost, setDragGhost] = useState<string | null>(null);
  /** Space tile currently under the dragged window (highlighted drop target). */
  const [dropTargetId, setDropTargetId] = useState<string | null>(null);
  /** True while the ambient tab has morphed into the "Teleport to Cua" box (the
   * drag's `prompt` phase). Drives the Ambient box + notch-height growth. */
  const [teleportPrompt, setTeleportPrompt] = useState(false);
  /** Keeps the portal window grown a beat past `teleportPrompt` so the box's
   * collapse animation is visible before the window shrinks back to the tab. */
  const [teleportTall, setTeleportTall] = useState(false);
  /** Whether Accessibility permission is still needed for window dragging. */
  const [axNeeded, setAxNeeded] = useState(false);
  /** Deferred window-shrink after a drag ends (so the box can animate out). */
  const teleportShrinkTimer = useRef<number | null>(null);
  // Latest state for handlers that need to look Spaces up without re-memoizing.
  const stateRef = useRef(state);
  stateRef.current = state;
  // Latest environment snapshot (monitor geometry) for the drag hit-tests.
  const envRef = useRef(env);
  envRef.current = env;
  const teleportRef = useRef(teleportState);
  teleportRef.current = teleportState;
  // A window-drag keeps the portal open even as our app loses focus.
  const draggingRef = useRef(false);
  // The dragged window's resolved app + whether a provider supports it.
  const dragAppRef = useRef<{
    app: LocalApp | null;
    entry: Record<string, unknown> | null;
    supported: boolean;
  } | null>(null);

  // Begin a teleport for a Space. In the native shell this opens the centered
  // screen-share-style picker window (label `teleport-picker`); in the browser
  // (design work / tests) it falls back to the in-portal sheet so the flow
  // stays previewable without Tauri. `app` pre-selects an app for the AX
  // window-drag / `.app`-drop shortcut, which jumps straight to consent.
  const beginTeleport = useCallback(
    (
      spaceId: string,
      spaceName: string,
      app?: LocalApp | null,
      entry?: Record<string, unknown> | null,
      files?: string[],
    ) => {
      if (teleportBridge.isNative) {
        teleportBridge
          .openPicker({ spaceId, spaceName, app: app ?? null, entry: entry ?? null, files: files ?? [] })
          .catch((error: unknown) => reportBridgeError("open teleport picker", error));
      } else {
        setTeleportState({
          spaceId,
          spaceName,
          preselect: entry ? { entry: entryFromCore(entry), files: files ?? [] } : null,
        });
      }
    },
    [teleportBridge],
  );
  const beginTeleportRef = useRef(beginTeleport);
  beginTeleportRef.current = beginTeleport;

  // Drop an app on the "+" tile: create a fresh Space in the default location
  // (Settings, `cua config set default.on`) and, once it answers, open the
  // teleport for it with the dragged app pre-selected: a one-gesture "new
  // computer with this app on it".
  const createSpaceAndTeleport = useCallback(
    async (app: LocalApp | null, entry: Record<string, unknown> | null = null) => {
      try {
        dispatch({ type: "notify", text: "Creating a Space…" });
        // No location: the shell creates it where the default location says,
        // from the default image.
        const id = await fleetSync.createSpace();
        const name = stateRef.current.spaces.find((s) => s.id === id)?.name ?? "New Space";
        beginTeleportRef.current(id, name, app, entry);
      } catch (error) {
        reportBridgeError("create Space", error);
        dispatch({ type: "notify", text: "Couldn’t create a Space" });
      }
    },
    [fleetSync],
  );
  const createSpaceAndTeleportRef = useRef(createSpaceAndTeleport);
  createSpaceAndTeleportRef.current = createSpaceAndTeleport;

  // Initial environment snapshot from the shell.
  useEffect(() => {
    let cancelled = false;
    bridge
      .getEnvironment()
      .then((e) => {
        if (!cancelled) setEnv(e);
      })
      .catch((error: unknown) => reportBridgeError("get environment", error));
    return () => {
      cancelled = true;
    };
  }, [bridge]);

  // The window size follows the logical mode, except that an in-flight drag
  // teleport grows the *ambient* window taller so the "Teleport to Cua" box has
  // room to hang below the notch. The selector/create modes already own the
  // screen, so they are unaffected.
  const effectiveMode: WindowMode =
    state.mode === "ambient" && teleportTall
      ? "ambient-teleport"
      : state.mode;

  // Keep the native window in step with the effective mode.
  useEffect(() => {
    let cancelled = false;
    bridge
      .setWindowMode(effectiveMode)
      .then((geometry) => {
        if (!cancelled) setEnv((prev) => (prev ? { ...prev, geometry } : prev));
      })
      .catch((error: unknown) => reportBridgeError(`set window mode to ${effectiveMode}`, error));
    return () => {
      cancelled = true;
    };
  }, [bridge, effectiveMode]);

  // Auto-collapse after a switch confirmation; auto-clear notices.
  useEffect(() => {
    if (!state.notice) return;
    const notice = state.notice;
    if (notice.kind === "switch") {
      collapseTimer.current = window.setTimeout(() => dispatch({ type: "collapse" }), collapseDelayMs);
    }
    noticeTimer.current = window.setTimeout(() => dispatch({ type: "clear-notice", id: notice.id }), NOTICE_MS);
    return () => {
      if (collapseTimer.current) window.clearTimeout(collapseTimer.current);
      if (noticeTimer.current) window.clearTimeout(noticeTimer.current);
    };
  }, [state.notice, collapseDelayMs]);

  // Losing window focus in the native shell tucks the portal away. Two things
  // survive a blur: an open teleport ("Sync to…") consent flow, and an active
  // window-drag — while the user drags an app window the portal stays up as a
  // drop surface even though dragging steals focus from us.
  useEffect(() => {
    if (!bridge.isNative) return;
    const onBlur = () => {
      if (teleportRef.current || draggingRef.current) return;
      dispatch({ type: "collapse" });
    };
    window.addEventListener("blur", onBlur);
    return () => window.removeEventListener("blur", onBlur);
  }, [bridge]);

  // AX-based window drag onto a Space (macOS). Dragging a supported app's
  // window first morphs the notch into the "Teleport to Cua" box (the ambient
  // tab collapses in, the notch grows taller); bringing the window up to the
  // notch expands the Space selector, and releasing over a tile opens the
  // teleport/sync flow with that app pre-selected. Screen coords arrive as
  // top-left logical points; we map them into the portal's client space to
  // hit-test our own tiles.
  useEffect(() => {
    if (!bridge.isNative) return;
    let cancelled = false;
    let unlisten: (() => void) | null = null;

    // The portal's top-left in logical points, refreshed as it moves/resizes.
    let origin: { x: number; y: number } | null = null;
    let originAt = 0;
    const refreshOrigin = async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        const [pos, scale] = await Promise.all([win.innerPosition(), win.scaleFactor()]);
        origin = { x: pos.x / scale, y: pos.y / scale };
        originAt = now();
      } catch {
        // window went away; keep the last origin
      }
    };
    const tileAt = (x: number, y: number): string | null => {
      if (!origin) return null;
      const client = screenToClient({ x, y }, origin);
      const element = document.elementFromPoint(client.x, client.y);
      const tile = element?.closest<HTMLElement>("[data-space-id]");
      return tile?.dataset.spaceId ?? null;
    };

    // Where the switcher opens: the app core's drag trigger over the
    // portal's display (the notch's bottom edge from the screen's safe area,
    // read by the shell). Before the shell answers (or off macOS), the
    // portal's monitor with a menu-bar-high notch stands in.
    let shellDisplays: DragDisplay[] = [];
    const displays = (): DragDisplay[] => {
      const m = envRef.current?.geometry.monitor;
      if (!m) return shellDisplays;
      const own = displayFor(shellDisplays, m);
      return own.length ? own : fallbackDragDisplays(m);
    };
    const refreshDisplays = () => {
      void Promise.resolve()
        .then(() => windowDragBridge.dragDisplays())
        .then((d) => {
          if (d.length) shellDisplays = d;
        })
        .catch(() => {});
    };
    refreshDisplays();

    // Drag state machine (item A). A supported window-drag first morphs the
    // notch into the "Teleport to Cua" box (the real window is never moved — the
    // ghost is a purely additive echo). Bringing the window up to the notch
    // expands the full Space selector and retires the box; pulling back collapses
    // to the box again. The two are never on screen together. The gesture is a
    // foreign window drag, so the portal hit-tests the global cursor here.
    const overlay = { current: initialDragOverlay as DragOverlayState };

    const runEffects = (effects: DragOverlayEffect[]) => {
      for (const effect of effects) {
        if (effect.kind === "capture") {
          const wid = effect.windowId;
          if (wid == null) continue;
          // Additive ghost capture (needs Screen Recording; null ⇒ no preview).
          // Nothing here hides, moves, or repositions the user's real window.
          void teleportBridge
            .captureThumbnail(wid)
            .then((url) => {
              const next = applyDragOverlay(overlay.current, { type: "ghost-ready", ghost: url ?? null });
              overlay.current = next.state;
              // Feeds the ghost into the ambient "Teleport to Cua" box.
              setDragGhost(overlay.current.ghost);
            })
            .catch(() => {});
        } else if (effect.kind === "commit") {
          const drag = dragAppRef.current;
          const space = stateRef.current.spaces.find((s) => s.id === effect.spaceId);
          if (space && space.status !== "local") {
            const ok = Boolean(drag?.supported);
            beginTeleportRef.current(
              space.id,
              space.name,
              ok ? (drag?.app ?? null) : null,
              ok ? (drag?.entry ?? null) : null,
            );
          }
        }
      }
    };
    const dispatchOverlay = (event: DragOverlayEvent) => {
      const next = applyDragOverlay(overlay.current, event);
      overlay.current = next.state;
      runEffects(next.effects);
    };

    // Expand from the box into the full Space selector (never both at once).
    const expandSelector = () => {
      dispatchOverlay({ type: "enter-notch" });
      dispatch({ type: "expand" });
      setDragActive(true);
      setTeleportPrompt(false); // the box retires; the selector owns the screen
      // Let the expand resize/reposition settle before hit-testing tiles.
      window.setTimeout(() => void refreshOrigin(), 120);
    };
    // Collapse the selector back to just the "Teleport to Cua" box.
    const collapseSelector = () => {
      dispatchOverlay({ type: "leave-notch" });
      dispatch({ type: "collapse" });
      setDragActive(false);
      setDropTargetId(null);
      setTeleportPrompt(true); // the notch box grows back
    };

    // The drag's end (a drop or a cancel): commit over a tile, dismiss both
    // surfaces. `x`/`y` is where it was released.
    const endDrag = (x: number | null, y: number | null) => {
      draggingRef.current = false;
      const finish = (id: string | null) => {
        const isNew = id === NEW_SPACE_DROP_ID;
        const space = id && !isNew ? stateRef.current.spaces.find((s) => s.id === id) : undefined;
        const committing = Boolean(space && space.status !== "local");
        // Capture the dragged app before the cleanup nulls it — the "+" drop
        // creates a Space asynchronously and teleports it in afterwards.
        const droppedApp =
          dragAppRef.current?.app && dragAppRef.current.supported ? dragAppRef.current.app : null;
        const droppedEntry = dragAppRef.current?.supported ? (dragAppRef.current.entry ?? null) : null;
        // `drop` commits over a valid tile (→ consent picker → push → item B)
        // and dismisses both surfaces. The real window is never moved.
        dispatchOverlay({ type: "drop", spaceId: committing ? id : null });
        // Always collapse the switcher on drop: on a commit the teleport now
        // takes over in the (smaller, centered) Space view, so the expanded
        // selector must not linger at the top over that window.
        dispatch({ type: "collapse" });
        setDragActive(false);
        setDragAppName(null);
        setDragGhost(null);
        setDropTargetId(null);
        dragAppRef.current = null;
        // Animate the box collapsing back to the ambient tab, then shrink the
        // portal window once that morph has finished.
        setTeleportPrompt(false);
        if (teleportShrinkTimer.current) window.clearTimeout(teleportShrinkTimer.current);
        teleportShrinkTimer.current = window.setTimeout(() => {
          setTeleportTall(false);
          teleportShrinkTimer.current = null;
        }, TELEPORT_SHRINK_MS);
        // Dropped on "+": create a new Space, then teleport the app into it.
        if (isNew) void createSpaceAndTeleportRef.current(droppedApp, droppedEntry);
      };
      // A tile can only be under the cursor while the selector is expanded.
      if (overlay.current.phase === "selector" && x != null && y != null) {
        void refreshOrigin().then(() => finish(tileAt(x, y)));
      } else {
        finish(null);
      }
    };

    // A confirmed move (the core told it from a resize): morph the notch
    // into the "Teleport to Cua" box.
    const beginDrag = (windowId: number | null, appName: string | null) => {
      draggingRef.current = true;
      // A fresh drag cancels any pending shrink from a previous one.
      if (teleportShrinkTimer.current) {
        window.clearTimeout(teleportShrinkTimer.current);
        teleportShrinkTimer.current = null;
      }
      // Selector stays collapsed until the window is brought up to the notch:
      // if it happened to be open already, collapse it so only the box shows.
      dispatch({ type: "collapse" });
      setDragActive(false);
      setDragAppName(appName);
      setDragGhost(null);
      setDropTargetId(null);
      // Grow the window, open the box, and kick off the additive ghost grab.
      dispatchOverlay({ type: "start", windowId, appName });
      setTeleportTall(true);
      setTeleportPrompt(true);
    };

    let trigger = initialDragTrigger();
    let tickTimer: number | null = null;
    // Runs one trigger event and the overlay events it decides on.
    const feed = (event: DragTriggerEvent, x: number | null, y: number | null) => {
      const t = applyDragTrigger(trigger, event, displays());
      trigger = t.state;
      for (const o of t.overlay) {
        if (o.type === "start") beginDrag(o.windowId, o.appName);
        else if (o.type === "enter-notch") expandSelector();
        else if (o.type === "leave-notch") collapseSelector();
        else if (o.type === "drop") endDrag(x, y);
        else if (o.type === "cancel") endDrag(null, null);
      }
      if (trigger.phase === "expanded" && event.type === "cursor") {
        // Over the selector: hit-test its tiles.
        if (now() - originAt > 250) void refreshOrigin();
        const id = tileAt(event.x, event.y);
        setDropTargetId(id);
        // The "+" tile is a drop target but not a real Space: keep the box's
        // "over <Space>" copy neutral while the cursor is over it.
        dispatchOverlay(
          id && id !== NEW_SPACE_DROP_ID ? { type: "over", spaceId: id } : { type: "out" },
        );
      }
      if (tickTimer != null) window.clearTimeout(tickTimer);
      tickTimer = null;
      if (t.tickAtMs != null) {
        tickTimer = window.setTimeout(() => {
          tickTimer = null;
          feed({ type: "tick", tMs: now() }, x, y);
        }, Math.max(0, t.tickAtMs - now()));
      }
    };

    const handle = (event: WindowDragEvent) => {
      const t = now();
      if (event.phase === "start") {
        // Only pop the drop surface for apps teleport can bring up in a Space
        // (full or install only, per the SDK catalog), and never while the
        // notch is hidden (menu bar only).
        if (!event.supported || menuBarRef.current) return;
        const app: LocalApp | null =
          event.appId && event.appName ? { id: event.appId, name: event.appName } : null;
        dragAppRef.current = { app, entry: event.entry ?? null, supported: Boolean(event.supported) };
        refreshDisplays();
        feed(
          {
            type: "start",
            windowId: event.windowId ?? null,
            appName: event.appName ?? null,
            x: event.x,
            y: event.y,
            tMs: t,
            startFrame: event.startFrame ?? null,
            frame: event.frame ?? null,
          },
          event.x,
          event.y,
        );
        return;
      }
      if (!trigger.active) return;
      if (event.frame) feed({ type: "frame", frame: event.frame }, event.x, event.y);
      feed(
        event.phase === "move"
          ? { type: "cursor", x: event.x, y: event.y, tMs: t }
          : { type: "end", x: event.x, y: event.y, tMs: t },
        event.x,
        event.y,
      );
    };

    windowDragBridge
      .onWindowDrag(handle)
      .then((stop) => {
        if (cancelled) stop();
        else unlisten = stop;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
      if (tickTimer != null) window.clearTimeout(tickTimer);
      // Never leave the portal grown/morphed if a drag was interrupted.
      if (teleportShrinkTimer.current) {
        window.clearTimeout(teleportShrinkTimer.current);
        teleportShrinkTimer.current = null;
      }
      setTeleportPrompt(false);
      setTeleportTall(false);
    };
  }, [bridge, windowDragBridge, teleportBridge, now]);

  // Track whether Accessibility permission is still needed for window dragging.
  useEffect(() => {
    if (!bridge.isNative) return;
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    windowDragBridge
      .axTrusted()
      .then((trusted) => {
        if (!cancelled) setAxNeeded(!trusted);
      })
      .catch(() => {});
    windowDragBridge
      .onPermission((granted) => {
        if (!cancelled) setAxNeeded(!granted);
      })
      .then((stop) => {
        if (cancelled) stop();
        else unlisten = stop;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [bridge, windowDragBridge]);

  const enableWindowDrag = useCallback(() => {
    // Prompts (System Settings → Privacy → Accessibility), then tries to
    // install the monitor; a live grant flips the banner off immediately.
    windowDragBridge
      .requestAxTrust()
      .catch(() => {})
      .finally(() => {
        void windowDragBridge
          .startWindowDrag()
          .then((active) => setAxNeeded(!active))
          .catch(() => {});
      });
  }, [windowDragBridge]);

  // Dropping a .app bundle on a Space tile starts the "Sync to…" consent flow
  // for that app and Space (macOS Finder/Dock drags deliver bundle paths).
  // Live-window drags are handled by the AX monitor above; this Finder-drop
  // path and the per-tile sync button are the other triggers.
  useEffect(() => {
    if (!bridge.isNative || state.mode !== "switcher") return;
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void import("@tauri-apps/api/webview").then(({ getCurrentWebview }) => {
      if (cancelled) return;
      void getCurrentWebview()
        .onDragDropEvent((event) => {
          if (event.payload.type !== "drop") return;
          const paths = event.payload.paths;
          if (!hasAppPath(paths)) return;
          const scale = window.devicePixelRatio || 1;
          const element = document.elementFromPoint(
            event.payload.position.x / scale,
            event.payload.position.y / scale,
          );
          const tile = element?.closest<HTMLElement>("[data-space-id]");
          const space = tile
            ? stateRef.current.spaces.find((s) => s.id === tile.dataset.spaceId)
            : undefined;
          if (!space || space.status === "local") return;
          void handleAppDrop(paths, space, beginTeleportRef.current, teleportAppsBridge);
        })
        .then((stop) => {
          if (cancelled) stop();
          else unlisten = stop;
        });
    });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [bridge, state.mode, teleportAppsBridge]);

  const setDisplayStyle = useCallback(
    (style: DisplayStyle) => {
      bridge
        .setDisplayStyle(style)
        .then(setEnv)
        .catch((error: unknown) => reportBridgeError(`set display style to ${style}`, error));
    },
    [bridge],
  );

  // Settings live in the main window; it writes them to local storage and
  // tells this window to re-read them.
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    void import("@tauri-apps/api/event")
      .then(({ listen }) => listen("settings:changed", () => setMenuBarState(readMenuBar())))
      .then((stop) => {
        if (cancelled) stop();
        else unlisten = stop;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, []);

  // New Space is a window (the main window's wizard), never a notch panel:
  // open it and fold the notch away.
  const openCreate = useCallback(() => {
    void spacesListBridge.openNewSpace().catch((error: unknown) => reportBridgeError("open New Space", error));
    dispatch({ type: "collapse" });
  }, [spacesListBridge]);

  // Dismiss the switcher. In the Island theme this first springs the panel back
  // into the notch (keeping the window at switcher size for the animation) and
  // only then collapses to the ambient tab; every other case collapses at once.
  const dismiss = useCallback(() => {
    if (theme !== "island" || stateRef.current.mode !== "switcher") {
      dispatch({ type: "collapse" });
      return;
    }
    if (islandCollapseTimer.current !== null) window.clearTimeout(islandCollapseTimer.current);
    setIslandCollapsing(true);
    const reduce =
      typeof window !== "undefined" &&
      window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
    islandCollapseTimer.current = window.setTimeout(
      () => {
        islandCollapseTimer.current = null;
        setIslandCollapsing(false);
        dispatch({ type: "collapse" });
      },
      reduce ? 0 : ISLAND_COLLAPSE_MS,
    );
  }, [theme]);

  // Never leave a pending spring-back timer running on unmount.
  useEffect(
    () => () => {
      if (islandCollapseTimer.current !== null) window.clearTimeout(islandCollapseTimer.current);
    },
    [],
  );

  // Global keyboard handling.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const meta = event.metaKey || event.ctrlKey;
      if (meta && event.shiftKey && event.key.toLowerCase() === "d" && import.meta.env.DEV && env) {
        event.preventDefault();
        setDisplayStyle(env.displayStyle === "notched" ? "no-notch" : "notched");
        return;
      }
      if (meta && event.key.toLowerCase() === "n") {
        event.preventDefault();
        openCreate();
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        if (teleportRef.current) {
          setTeleportState(null);
          return;
        }
        if (state.mode === "create-fleet") dispatch({ type: "cancel-create" });
        else dismiss();
        return;
      }
      if (state.mode !== "switcher") return;
      switch (event.key) {
        case "ArrowRight":
        case "ArrowDown":
          event.preventDefault();
          dispatch({ type: "focus-move", delta: 1 });
          break;
        case "ArrowLeft":
        case "ArrowUp":
          event.preventDefault();
          dispatch({ type: "focus-move", delta: -1 });
          break;
        case "Home":
          event.preventDefault();
          dispatch({ type: "focus", index: 0 });
          break;
        case "End":
          event.preventDefault();
          dispatch({ type: "focus", index: NEW_TILE_INDEX });
          break;
        case "Enter": {
          // Let real buttons handle Enter themselves when they own focus.
          const target = event.target as HTMLElement | null;
          if (target && target.closest("[data-owns-enter]")) return;
          event.preventDefault();
          if (state.focusIndex === NEW_TILE_INDEX) openCreate();
          else {
            const space = state.spaces[state.focusIndex];
            if (space) dispatch({ type: "select", id: space.id, now: now() });
          }
          break;
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [state.mode, state.focusIndex, state.spaces, now, env, setDisplayStyle, openCreate, dismiss]);

  const displayStyle: DisplayStyle = env?.displayStyle ?? "no-notch";
  const handlers = useMemo(
    () => ({
      expand: () => dispatch({ type: "expand" }),
      collapse: () => dispatch({ type: "collapse" }),
      openCreate,
      cancelCreate: () => dispatch({ type: "cancel-create" }),
      focus: (index: number) => dispatch({ type: "focus", index }),
      select: (id: string) => {
        dispatch({ type: "select", id, now: now() });
        // Switching to a cloud Space focuses (or opens) its desktop window —
        // macOS then handles the actual Space switch when it is fullscreen.
        const space = stateRef.current.spaces.find((s) => s.id === id);
        if (!space || !fleetBridge.isNative) return;
        const request = viewerRequestFor(space);
        if (request) {
          fleetBridge.openSpaceWindow(request).catch((error: unknown) => {
            reportBridgeError(`open Space window for ${id}`, error);
          });
        }
      },
      pin: (space: Space) => {
        if (!fleetBridge.isNative) return;
        const request = viewerRequestFor(space);
        if (request) {
          fleetBridge.pinSpacePip(request).catch((error: unknown) => {
            reportBridgeError(`pin Space ${space.id}`, error);
          });
        }
      },
      shareApp: (space: Space) => {
        if (space.status === "local") return;
        beginTeleport(space.id, space.name);
      },
      createFleet: (draft: FleetDraft) => dispatch({ type: "create-fleet", draft, now: now() }),
    }),
    [now, openCreate, fleetBridge, beginTeleport],
  );

  // The popped-out Spaces list window owns no Spaces state, so its whole-Space
  // actions (stream the desktop, pin as PiP) come back here as events.
  useEffect(() => {
    if (!fleetBridge.isNative) return;
    let cancelled = false;
    const stops: Array<() => void> = [];
    void import("@tauri-apps/api/event")
      .then(async ({ listen }) => {
        const bind = async (event: string, run: (spaceId: string) => void) => {
          const stop = await listen<{ spaceId: string }>(event, (message) => {
            const id = message.payload?.spaceId;
            if (id) run(id);
          });
          if (cancelled) stop();
          else stops.push(stop);
        };
        await bind("spaces-list:stream-desktop", (id) => handlers.select(id));
        await bind("spaces-list:pin", (id) => {
          const space = stateRef.current.spaces.find((s) => s.id === id);
          if (space) handlers.pin(space);
        });
        // Teleport opens the existing teleport picker for that Space, which is
        // where the app is chosen and consented to — the same surface a tile's
        // share button and an app drop use.
        await bind("spaces-list:teleport", (id) => {
          const space = stateRef.current.spaces.find((s) => s.id === id);
          if (space) handlers.shareApp(space);
        });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      for (const stop of stops) stop();
    };
  }, [fleetBridge, handlers]);


  return (
    <FleetContext.Provider value={fleetContext}>
      <div
        className="portal"
        data-mode={state.mode}
        data-display={displayStyle}
        data-theme={theme}
        data-collapsing={islandCollapsing ? "true" : undefined}
        data-notch-hover={notchHover && !menuBar ? "on" : "off"}
        onMouseDown={(event) => {
          // In the open selector, clicking the notch cap above it or the shadow /
          // empty background below it dismisses — same as clicking off it — but
          // never a click on a tile, the filter box, or another control.
          if (state.mode !== "switcher") return;
          const target = event.target as HTMLElement;
          if (
            target.closest(
              "button, a, input, textarea, select, [role='option'], [data-owns-enter], .tile, .switcher-search",
            )
          )
            return;
          dismiss();
        }}
      >
        {displayStyle === "notched" && !(menuBar && state.mode === "ambient") && (
          <NotchCap mode={state.mode} teleport={teleportTall} />
        )}


        {state.mode === "ambient" && (
          <Ambient
            spaces={state.spaces}
            displayStyle={displayStyle}
            onExpand={handlers.expand}
            teleportOpen={teleportPrompt}
            menuBar={menuBar}
            hotspotActive={hotspot.active}
            transfer={transfer}
          />
        )}

        {state.mode === "switcher" && (
          <Switcher
            spaces={state.spaces}
            selectedId={state.selectedId}
            focusIndex={state.focusIndex}
            notice={state.notice}
            onFocus={handlers.focus}
            onSelect={handlers.select}
            onNew={handlers.openCreate}
            onCollapse={dismiss}
            onPin={fleetSync.live || fleetBridge.isNative ? handlers.pin : undefined}
            onShareApp={handlers.shareApp}
            onDelete={fleetSync.live ? fleetSync.deleteSpace : undefined}
            onPower={fleetSync.live ? fleetSync.setPower : undefined}
            hotspotActive={hotspot.active}
            hotspotSpaceId={hotspot.spaceId}
            onStopHotspot={stopHotspot}
            dropMode={dragActive}
            dragAppName={dragAppName}
            dragGhost={dragGhost}
            dropTargetId={dropTargetId}
            spacesList={spacesListBridge}
            footer={
              axNeeded ? (
                <button type="button" className="ax-prompt" data-owns-enter onClick={enableWindowDrag}>
                  Enable window dragging in System Settings → Privacy → Accessibility
                </button>
              ) : null
            }
          />
        )}

        {state.mode === "switcher" && teleportState && (
          <div className="teleport-backdrop" role="presentation" onClick={() => setTeleportState(null)}>
            <section
              className="teleport-sheet"
              role="dialog"
              aria-modal="true"
              aria-label={`Teleport an app to ${teleportState.spaceName}`}
              onClick={(event) => event.stopPropagation()}
            >
              <AppTeleportPicker
                host={teleportAppsBridge.host(teleportState.spaceId)}
                spaceName={teleportState.spaceName}
                preselect={teleportState.preselect}
                onClose={() => setTeleportState(null)}
              />
            </section>
          </div>
        )}

      </div>
    </FleetContext.Provider>
  );
}
