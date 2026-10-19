// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Live desktop viewer for one Space, rendered in its own Tauri window
 * (`space-*` for the interactive fullscreen-capable window, `pip-*` for the
 * small view-only picture-in-picture mirror).
 *
 * The desktop streams over the spacesd's media plane (rcdp wire v2): the
 * shell mints a scoped ticket for a `display` target (`open_space_stream`)
 * and `MediaCanvas` plays it — H.264/BGRA/PNG video plus desktop audio.
 * The desktop stream is the only transport: a Space whose spacesd lacks
 * `desktop_stream` shows an "update the image" message instead.
 */

import {
  useEffect,
  useMemo,
  useReducer,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";

import { FEATURE } from "../model/spaces";
import {
  reduceTransfer,
  transferProgress,
  transferSizeLabel,
  transferTitle,
  type TransferOverlayState,
} from "../model/transfer";
import type { FleetBridge, HotspotStatus, ViewerConfig } from "../native/fleet";
import { createTransferBridge, type TransferBridge } from "../native/transfer";
import { HotspotGlyph } from "../components/HotspotGlyph";
import type { MediaStatus, MediaTicket } from "@cua/spacesd-html5/core/mediaSession";
import { MediaCanvas, toMediaTicket } from "./WindowStream";
import { useAppDropTeleport } from "./useAppDrop";
import { PresenceOverlay } from "./presence/PresenceOverlay";

interface SpaceViewerProps {
  fleet: FleetBridge;
  /** Transfer overlay bridge; defaults to the environment-appropriate one. */
  transfer?: TransferBridge;
}

type Boot =
  | { kind: "loading" }
  | { kind: "error"; message: string }
  | { kind: "ready"; config: ViewerConfig; desktopStream: boolean };

/** Shown when a Space's cua-spacesd has no desktop capture. */
export const NO_DESKTOP_STREAM_MESSAGE =
  "This Space's cua-spacesd has no desktop capture. Update the Space's image to view its desktop.";

/** Whether the Space can stream its desktop. False only when the registry
 * says its spacesd lacks `desktop_stream`; an unknown Space (registry
 * unavailable) still tries the stream. */
export async function hasDesktopStream(fleet: FleetBridge, spaceId: string): Promise<boolean> {
  try {
    const row = fleet.spaceInfo
      ? await fleet.spaceInfo(spaceId)
      : (await fleet.listSpaces()).find((r) => r.id === spaceId);
    if (row && !row.features.includes(FEATURE.desktopStream)) return false;
  } catch {
    // registry unavailable: try the stream
  }
  return true;
}

/**
 * Coding agents offered by the fullscreen bar's "Launch Agent" dropdown, in
 * display order. `id` is the cua-agents harness id. `launch` picks the path:
 *
 * - `teleport`: open the teleport picker pre-targeted to the agent's teleport
 *   provider, which moves the user's signed-in session into the Space behind
 *   the consent sheet.
 * - `terminal`: install the agent's pinned CLI in the Space, then open it in a
 *   terminal on the Space's desktop. The user signs in there; nothing on this
 *   Mac is read or copied.
 *
 * `ready: false` keeps an entry listed with an honest "coming soon" notice.
 */
interface CodingAgent {
  id: string;
  name: string;
  ready: boolean;
  launch: "teleport" | "terminal";
}

const CODING_AGENTS: readonly CodingAgent[] = [
  // Claude Code has a teleport provider: the guest-side receiver writes the
  // consented login and launches `claude`.
  { id: "claude-code", name: "Claude Code", ready: true, launch: "teleport" },
  { id: "openai-codex", name: "OpenAI Codex", ready: true, launch: "terminal" },
  { id: "gemini-cli", name: "Gemini CLI", ready: true, launch: "terminal" },
  { id: "opencode", name: "OpenCode", ready: true, launch: "terminal" },
  { id: "goose", name: "Goose", ready: true, launch: "terminal" },
  { id: "pi", name: "Pi", ready: true, launch: "terminal" },
  { id: "hermes", name: "Hermes", ready: true, launch: "terminal" },
  { id: "openclaw", name: "OpenClaw", ready: true, launch: "terminal" },
  // Google Antigravity is not listed: it has no interactive terminal CLI (only
  // a headless ACP server), so it runs through the SDK, `cua agent run` and MCP.
];

export function SpaceViewer({ fleet, transfer }: SpaceViewerProps) {
  const [boot, setBoot] = useState<Boot>({ kind: "loading" });
  const transferBridge = useMemo(() => transfer ?? createTransferBridge(), [transfer]);
  // An app dropped on the Space window opens "Teleport an app…" for it.
  useAppDropTeleport(boot.kind === "ready" ? boot.config.space : null);

  useEffect(() => {
    let cancelled = false;
    fleet
      .viewerConfig()
      .then(async (config) => {
        const desktopStream = await hasDesktopStream(fleet, config.space.id);
        if (!cancelled) setBoot({ kind: "ready", config, desktopStream });
      })
      .catch((error: unknown) => {
        if (!cancelled)
          setBoot({
            kind: "error",
            message: error instanceof Error ? error.message : String(error),
          });
      });
    return () => {
      cancelled = true;
    };
  }, [fleet]);

  if (boot.kind === "loading") {
    return <div className="viewer viewer-message">Connecting…</div>;
  }
  if (boot.kind === "error") {
    return <div className="viewer viewer-message viewer-error">{boot.message}</div>;
  }
  return (
    <ViewerSession
      fleet={fleet}
      transfer={transferBridge}
      config={boot.config}
      desktopStream={boot.desktopStream}
    />
  );
}

type Phase = "connecting" | "connected" | "disconnected" | "failed" | "unsupported";

/** Media status → the viewer's phase. */
export function phaseForStatus(status: MediaStatus): Phase {
  switch (status) {
    case "streaming":
      return "connected";
    case "ended":
      return "disconnected";
    case "failed":
      return "failed";
    default:
      return "connecting";
  }
}

function ViewerSession({
  fleet,
  transfer,
  config,
  desktopStream,
}: {
  fleet: FleetBridge;
  transfer: TransferBridge;
  config: ViewerConfig;
  desktopStream: boolean;
}) {
  const [phase, setPhase] = useState<Phase>(desktopStream ? "connecting" : "unsupported");
  // Bumped by Retry/Reconnect to force a fresh session.
  const [attempt, setAttempt] = useState(0);
  // The remote desktop's pixel size (authoritative geometry from the stream),
  // for the aspect lock and the PiP sizing.
  const [frameSize, setFrameSize] = useState<{ width: number; height: number } | null>(null);

  const { space, view } = config;
  const openDesktop = useMemo(
    () => (): Promise<MediaTicket> =>
      fleet
        .openStream(
          space.id,
          { kind: "display" },
          view === "pip" ? { policy: "view_only", audio: false, maxDimension: 1280 } : { audio: true },
        )
        .then(toMediaTicket),
    [fleet, space.id, view],
  );
  // Item D: a recent screenshot (from the shared cache, via viewer_config)
  // paints a blurred background while the live stream is connecting.
  const blurredBg = config.lastScreenshot ?? null;

  // Item B: the teleport transfer overlay, seeded from viewer_config (active on
  // mount if a transfer is in flight) and advanced by `space-transfer` events.
  const [overlay, dispatchOverlay] = useReducer(
    reduceTransfer,
    config.transfer ?? null,
    (seed): TransferOverlayState | null => (seed ? { ...seed } : null),
  );
  useEffect(() => {
    if (view !== "space") return;
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    transfer
      .onTransfer((signal) => {
        if (!cancelled) dispatchOverlay(signal);
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
  }, [transfer, view]);
  const retryTransfer = () => {
    // Optimistically return to the active state; the shell (Rust) re-runs the
    // push from the params it stored when the transfer began.
    dispatchOverlay({ status: "start", appName: overlay?.appName });
    void transfer.retry(space.id).catch(() => {});
  };
  const cancelTransfer = () => {
    // Dismiss the overlay back to the live stream; Rust forgets the retry params.
    dispatchOverlay({ status: "done" });
    void transfer.cancel(space.id).catch(() => {});
  };

  // Fullscreen top-bar (item: notch bar) transient notice: a short-lived
  // inline message for bar actions (for example a failed "stop sharing").
  const [topbarNotice, setTopbarNotice] = useState<string | null>(null);
  useEffect(() => {
    if (!topbarNotice) return;
    const timer = window.setTimeout(() => setTopbarNotice(null), 2600);
    return () => window.clearTimeout(timer);
  }, [topbarNotice]);
  const openTeleport = () => {
    void fleet.openTeleportPicker(space).catch(() => {});
  };

  // The "Launch Agent" dropdown: a button in the bar that opens a small dark
  // popover of coding agents above it. Esc / click-outside close it.
  const [agentMenuOpen, setAgentMenuOpen] = useState(false);
  const agentMenuRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (!agentMenuOpen) return;
    const onPointerDown = (event: MouseEvent) => {
      if (agentMenuRef.current && !agentMenuRef.current.contains(event.target as Node)) {
        setAgentMenuOpen(false);
      }
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setAgentMenuOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [agentMenuOpen]);
  // Optional arrow-key roving between menu items (Enter/Space fire the button
  // natively since each item is a real <button>).
  const onAgentMenuKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    event.preventDefault();
    const items = Array.from(
      agentMenuRef.current?.querySelectorAll<HTMLButtonElement>('[role="menuitem"]') ?? [],
    );
    if (items.length === 0) return;
    const current = items.indexOf(document.activeElement as HTMLButtonElement);
    const next =
      event.key === "ArrowDown"
        ? (current + 1) % items.length
        : (current - 1 + items.length) % items.length;
    items[next]?.focus();
  };
  // The agent being installed and opened in a terminal. Its notice stays up
  // for the whole install (minutes on a cold Space), unlike `topbarNotice`.
  const [agentInstalling, setAgentInstalling] = useState<string | null>(null);
  const selectAgent = (agent: CodingAgent) => {
    setAgentMenuOpen(false);
    if (!agent.ready) {
      setTopbarNotice(`${agent.name} support is coming soon`);
      return;
    }
    if (agent.launch === "teleport") {
      // Straight to the teleport consent sheet for this agent's provider.
      void fleet.launchAgent(space, agent.id, agent.name).catch(() => {});
      return;
    }
    if (agentInstalling) {
      setTopbarNotice(`Still installing ${agentInstalling}`);
      return;
    }
    setAgentInstalling(agent.name);
    fleet
      .launchAgentTerminal(space, agent.id)
      .then(() => setTopbarNotice(`${agent.name} is open in a terminal in this Space; sign in there`))
      .catch((error: unknown) =>
        setTopbarNotice(
          `Couldn't open ${agent.name}: ${error instanceof Error ? error.message : String(error)}`,
        ),
      )
      .finally(() => setAgentInstalling(null));
  };

  // Whether this Mac is currently sharing its network with THIS Space. Sharing
  // is turned ON from the teleport picker's "Network Fingerprint" option; here
  // in the viewer we only offer to stop it (a centered button in the bar).
  const [hotspotOn, setHotspotOn] = useState(false);
  useEffect(() => {
    let cancelled = false;
    let unsub: (() => void) | undefined;
    const reflect = (status: HotspotStatus) =>
      setHotspotOn(status.active && status.spaceId === space.id);
    void fleet
      .hotspotStatus()
      .then((status) => {
        if (!cancelled) reflect(status);
      })
      .catch(() => {});
    void fleet.onHotspotChanged(reflect).then((off) => {
      if (cancelled) off();
      else unsub = off;
    });
    return () => {
      cancelled = true;
      unsub?.();
    };
  }, [fleet, space.id]);
  const stopHotspot = () => {
    void fleet.stopHotspot().catch(() => setTopbarNotice("Couldn't stop sharing"));
  };

  // Keep the Space window at the remote desktop's aspect ratio while windowed.
  // The Space window is an ordinary resizable window: the remote display is
  // scaled to fit it (letterboxed), or shown 1:1 with scrolling. No aspect
  // lock and no forced fullscreen.
  const [scaleMode, setScaleMode] = useState<"fit" | "actual">("fit");
  useEffect(() => {
    if (view !== "space") return;
    const onKey = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey)) return;
      if (event.key === "0") {
        event.preventDefault();
        setScaleMode("actual");
      } else if (event.key === "9") {
        event.preventDefault();
        setScaleMode("fit");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [view]);

  // PiP mirror: tell the shell the remote desktop's pixel dimensions so it sizes
  // the window's *content* area (inner, excluding the title bar) to that aspect
  // and locks it NATIVELY (macOS contentAspectRatio) — smooth aspect-constrained
  // resize from every corner, no letterboxing. The stream reports its geometry
  // after "connected" (and again if the remote resolution changes); re-apply
  // only when the resolution changes.
  useEffect(() => {
    if (view !== "pip" || phase !== "connected" || !frameSize) return;
    void fleet.setPipAspect(space.id, frameSize.width, frameSize.height).catch(() => {});
  }, [view, phase, frameSize, fleet, space.id]);

  // The PiP window is FullScreenAuxiliary, so its green traffic-light zooms
  // (Tauri reports it as maximized) rather than entering fullscreen. Repurpose
  // that gesture: zooming the mirror promotes it to the full INTERACTIVE stream
  // (the Space window) and closes the mirror — no redundant in-content button.
  useEffect(() => {
    if (view !== "pip") return;
    let disposed = false;
    let wasMaximized = false;
    let unlisten: (() => void) | undefined;
    void (async () => {
      const { getCurrentWindow } = await import("@tauri-apps/api/window");
      const win = getCurrentWindow();
      unlisten = await win.onResized(() => {
        void win.isMaximized().then((maximized) => {
          if (disposed || maximized === wasMaximized) return;
          wasMaximized = maximized;
          if (maximized) {
            void fleet.openSpaceWindow(space).catch(() => {});
            void fleet.unpinSpacePip(space.id).catch(() => {});
          }
        });
      });
    })().catch(() => {
      // Not inside the Tauri shell (browser/test): no window to watch.
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [view, fleet, space, space.id]);

  const retry = () => setAttempt((current) => current + 1);

  return (
    <div className={`viewer viewer-${view}`} data-phase={phase}>
      {view === "space" && (
        // A solid black bar pinned over the notch strip. The live stream fills
        // the whole window UNDER it (goes under the notch); this bar overlays
        // the top strip so the notch sits invisibly within it. Buttons flank
        // the centered notch via space-between.
        <div className="viewer-topbar" role="toolbar" aria-label="Space controls">
          <div className="viewer-agent-menu" ref={agentMenuRef}>
            <button
              type="button"
              className="viewer-topbar-btn viewer-agent-menu-trigger"
              aria-haspopup="menu"
              aria-expanded={agentMenuOpen}
              onClick={() => setAgentMenuOpen((open) => !open)}
              title="Launch a coding agent for this Space"
            >
              Launch Agent
            </button>
            {agentMenuOpen && (
              <div
                className="viewer-agent-menu-list"
                role="menu"
                aria-label="Coding agents"
                onKeyDown={onAgentMenuKeyDown}
              >
                {CODING_AGENTS.map((agent) => (
                  <button
                    key={agent.id}
                    type="button"
                    role="menuitem"
                    className="viewer-agent-menu-item"
                    onClick={() => selectAgent(agent)}
                  >
                    {agent.name}
                  </button>
                ))}
              </div>
            )}
          </div>
          {hotspotOn && (
            <button
              type="button"
              className="viewer-hotspot-stop"
              onClick={stopHotspot}
              title="Stop sharing your network with this Space"
            >
              <HotspotGlyph className="viewer-hotspot-stop-glyph" />
              Stop sharing network
            </button>
          )}
          <div className="viewer-topbar-right">
            <button
              type="button"
              className="viewer-topbar-btn"
              aria-pressed={scaleMode === "actual"}
              onClick={() => setScaleMode((m) => (m === "fit" ? "actual" : "fit"))}
              title={scaleMode === "fit" ? "Show at actual size (⌘0)" : "Scale to fit the window (⌘9)"}
            >
              {scaleMode === "fit" ? "Actual Size" : "Fit to Window"}
            </button>
            <button
              type="button"
              className="viewer-topbar-btn"
              onClick={openTeleport}
              title={`Teleport an app into ${space.name}`}
            >
              Teleport an app…
            </button>
          </div>
          {(agentInstalling || topbarNotice) && (
            <span className="viewer-topbar-notice" role="status" aria-live="polite">
              {agentInstalling ? `Installing ${agentInstalling} in this Space…` : topbarNotice}
            </span>
          )}
        </div>
      )}
      {blurredBg && phase !== "connected" && (
        <div
          className="viewer-bg"
          aria-hidden="true"
          style={{ backgroundImage: `url("${blurredBg}")` }}
        />
      )}
      <div
        className="viewer-screen"
        data-scale={view === "space" ? scaleMode : "fit"}
        style={
          frameSize
            ? ({
                "--frame-w": `${frameSize.width}`,
                "--frame-h": `${frameSize.height}`,
                "--frame-dpr": `${window.devicePixelRatio || 1}`,
              } as React.CSSProperties)
            : undefined
        }
      >
        {phase !== "unsupported" && (
          <MediaCanvas
            className="viewer-canvas"
            open={openDesktop}
            interactive={view === "space"}
            audio={view === "space"}
            generation={attempt}
            onStatus={(status, detail) => {
              // A spacesd without desktop capture: say so instead of retrying.
              if (status === "failed" && detail && /desktop_stream|capability/i.test(detail)) {
                setPhase("unsupported");
                return;
              }
              setPhase(phaseForStatus(status));
            }}
            onGeometry={(width, height) => setFrameSize({ width, height })}
          />
        )}
        {phase !== "unsupported" && <PresenceOverlay spaceId={space.id} interactive={view === "space"} live={phase === "connected"} />}
      </div>
      {phase !== "connected" && (
        <div className="viewer-status">
          {phase === "connecting" && <span>Connecting to {space.name}…</span>}
          {phase === "disconnected" && (
            <>
              <span>Stream ended.</span>
              <button type="button" className="ghost-button" onClick={retry}>
                Reconnect
              </button>
            </>
          )}
          {phase === "failed" && (
            <>
              <span>Could not reach the desktop stream.</span>
              <button type="button" className="ghost-button" onClick={retry}>
                Retry
              </button>
            </>
          )}
          {phase === "unsupported" && <span role="alert">{NO_DESKTOP_STREAM_MESSAGE}</span>}
        </div>
      )}
      {view === "space" && overlay && (
        <TransferOverlayView state={overlay} onRetry={retryTransfer} onCancel={cancelTransfer} />
      )}
    </div>
  );
}

/**
 * The teleport transfer overlay (item B): a blurred frosted panel over the live
 * Space view. Once the CLI reports real upload progress it shows a DETERMINATE
 * bar (width = sent / total) with a "{x} MB / {y} MB" label; before any bytes
 * (the brief pre-upload/export phase) and under prefers-reduced-motion it falls
 * back to an indeterminate shimmer sweep. The error phase offers Retry (re-runs
 * the push in Rust) and Cancel (dismisses the overlay back to the live stream).
 * The parent clears the overlay when the transfer finishes (it fades out on done).
 */
export function TransferOverlayView({
  state,
  onRetry,
  onCancel,
}: {
  state: TransferOverlayState;
  onRetry: () => void;
  onCancel: () => void;
}) {
  const isError = state.phase === "error";
  const progress = transferProgress(state);
  const sizeLabel = transferSizeLabel(state);
  return (
    <div className="viewer-transfer" role="status" aria-live="polite" data-phase={state.phase}>
      <div className="viewer-transfer-card">
        <h2 className="viewer-transfer-title">{transferTitle(state)}</h2>
        {isError ? (
          <>
            <p className="viewer-transfer-error">
              {state.message || "The transfer did not complete."}
            </p>
            <div className="viewer-transfer-actions">
              <button type="button" className="ghost-button" onClick={onRetry}>
                Retry
              </button>
              <button
                type="button"
                className="ghost-button viewer-transfer-cancel"
                onClick={onCancel}
              >
                Cancel
              </button>
            </div>
          </>
        ) : progress !== null ? (
          <>
            <div
              className="viewer-transfer-bar"
              data-testid="transfer-bar"
              data-determinate="true"
              role="progressbar"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.round(progress * 100)}
            >
              <span
                className="viewer-transfer-fill"
                data-testid="transfer-fill"
                style={{ width: `${(progress * 100).toFixed(1)}%` }}
              />
            </div>
            <p className="viewer-transfer-hint" data-testid="transfer-size">
              {sizeLabel}
            </p>
          </>
        ) : (
          <>
            <div className="viewer-transfer-bar" data-testid="transfer-bar" aria-hidden="true">
              <span className="viewer-transfer-sweep" />
            </div>
            <p className="viewer-transfer-hint">Transferring the session…</p>
          </>
        )}
      </div>
    </div>
  );
}
