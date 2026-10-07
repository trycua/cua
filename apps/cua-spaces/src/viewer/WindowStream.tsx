// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * Per-window streaming (rcdp wire v2), hosted in a `win-*` (multi) or
 * `winone-*` (single) Tauri window, plus the reusable `MediaCanvas` the
 * Space viewer uses for the whole-desktop stream.
 *
 * Session setup is gRPC in the shell: `open_space_stream` returns a scoped
 * media ticket (`wsUrl` carries it, so no token ever reaches the webview and
 * no loopback bridge is needed for direct/local Spaces; Cua Cloud Spaces get a
 * `cua daemon` media-bridge ticket). `MediaSession` attaches and plays.
 *
 * - SINGLE mode (`targetWindow` set): one remote window fills the OS window,
 *   which is sized to the remote and resize-synced when geometry control is
 *   granted. An MCP-driven `stream_space_window` hands over a ready ticket in
 *   `targetWindow.mediaUrl`, which is attached as-is.
 * - MULTI mode: the Space's windows are polled (`list_remote_windows`) and
 *   each gets its own media session in a draggable panel.
 */

import { useEffect, useRef, useState } from "react";

import type { RemoteWindow } from "../model/teleport";
import type {
  FleetBridge,
  StreamOpts,
  StreamTarget,
  StreamTicketInfo,
  ViewerConfig,
} from "../native/fleet";
import { createTeleportBridge } from "../native/teleport";
import { telemetryBridge, type TelemetryBridge } from "../native/telemetry";
import { PresenceOverlay } from "./presence/PresenceOverlay";
import { MediaSession, type MediaStatus, type MediaTicket, type SessionOpenedPayload } from "@cua/spacesd-html5/core/mediaSession";

export { applyCursorShape, cursorShapeToCss } from "./cursorShape";

/** Separates the shell's window title from the live media session id stamped
 * onto it (single mode). Exported so tests can assert the composed title. */
export const SESSION_TITLE_SEP = " · session ";

/** Debounce (ms) on the OS-window resize → guest `set_window_geometry` echo. */
export const RESIZE_DEBOUNCE_MS = 150;
/** Multi mode: window-list poll cadence and the session cap. */
export const WINDOW_POLL_MS = 3_000;
export const MAX_WINDOW_SESSIONS = 16;

/**
 * A replica (shift-click second stream of one window) never asks for h264:
 * WKWebView admits one page at a time to WebCodecs video decoding, so a second
 * window's decoder would silently output nothing. PNG/BGRA paint without
 * WebCodecs; the replica is capped to keep raw frames affordable.
 */
export const REPLICA_OPTS: StreamOpts = {
  codecs: ["png", "bgra"],
  maxFps: 15,
  maxDimension: 1024,
  policy: "view_only",
};
/** H.264 first: the host takes the client's first servable codec. */
export const PRIMARY_WINDOW_OPTS: StreamOpts = {
  codecs: ["h264", "bgra", "png"],
  maxFps: 60,
  maxDimension: 4096,
  policy: "background_only",
};

/** A shell ticket → what `MediaSession` attaches to. */
export function toMediaTicket(info: StreamTicketInfo): MediaTicket {
  return { wsUrl: info.wsUrl, ticketExpiresAt: info.ticketExpiresAt, mediaSessionId: info.mediaSessionId };
}

// ---------------------------------------------------------------- MediaCanvas

export interface MediaCanvasProps {
  /** Mint a ticket (initially, on expiry and on 4401). */
  open?: () => Promise<MediaTicket>;
  /** A ready ticket to attach first (borrowed tickets have no `open`). */
  ticket?: MediaTicket;
  interactive: boolean;
  audio: boolean;
  className?: string;
  onStatus?: (status: MediaStatus, detail?: string) => void;
  onGeometry?: (width: number, height: number) => void;
  onOpened?: (opened: SessionOpenedPayload) => void;
  onTitle?: (title: string) => void;
  onGone?: (reason: string) => void;
  /** Receives the live session (for geometry sync), or null on teardown. */
  sessionRef?: (session: MediaSession | null) => void;
  /** Bump to force a fresh attach (user "Retry"). */
  generation?: number;
  /** Where the stream summary goes when the session ends (tests). */
  telemetry?: TelemetryBridge;
}

/** One media session drawn into a canvas that CSS scales to its box. */
export function MediaCanvas(props: MediaCanvasProps) {
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  // Callbacks change identity every render; the session reads the latest.
  const latest = useRef(props);
  latest.current = props;

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    let session: MediaSession | null = null;
    let cancelled = false;
    // Stream-stats telemetry: coarse buckets once per session, on teardown.
    let startedAt = 0;
    let height = 0;
    const cb = () => latest.current;
    const start = (ticket: MediaTicket) => {
      if (cancelled) return;
      startedAt = performance.now();
      session = new MediaSession({
        canvas,
        ticket,
        reopen: cb().open,
        interactive: cb().interactive,
        audio: cb().audio,
        onStatus: (s, d) => cb().onStatus?.(s, d),
        onGeometry: (w, h) => {
          height = Math.max(height, h);
          cb().onGeometry?.(w, h);
        },
        onOpened: (o) => cb().onOpened?.(o),
        onTitle: (t) => cb().onTitle?.(t),
        onGone: (r) => cb().onGone?.(r),
      });
      cb().sessionRef?.(session);
      session.start();
    };
    cb().onStatus?.("connecting");
    if (props.ticket) start(props.ticket);
    else if (props.open) {
      props
        .open()
        .then(start)
        .catch((error: unknown) => {
          if (!cancelled) cb().onStatus?.("failed", error instanceof Error ? error.message : String(error));
        });
    } else {
      cb().onStatus?.("failed", "no stream ticket");
    }
    return () => {
      cancelled = true;
      if (session) {
        const stats = session.stats;
        (cb().telemetry ?? telemetryBridge()).recordStream({
          codec: stats.codec,
          frames: stats.framesReceived,
          height,
          durationMs: performance.now() - startedAt,
        });
      }
      session?.stop();
      cb().sessionRef?.(null);
    };
    // The ticket source and interactivity define the session; callbacks are read live.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.ticket?.wsUrl, props.open, props.interactive, props.audio, props.generation]);

  return <canvas ref={canvasRef} className={props.className ?? "ws-frame-canvas"} tabIndex={0} />;
}

// ---------------------------------------------------------------- WindowStream

interface WindowStreamProps {
  fleet: FleetBridge;
  /** The Space's window list (multi mode); defaults to the shell's. */
  listWindows?: (spaceId: string) => Promise<RemoteWindow[]>;
}

type Boot =
  | { kind: "loading" }
  | { kind: "error"; message: string }
  | { kind: "ready"; config: ViewerConfig };

export function WindowStream({ fleet, listWindows }: WindowStreamProps) {
  const [boot, setBoot] = useState<Boot>({ kind: "loading" });

  useEffect(() => {
    let cancelled = false;
    fleet
      .viewerConfig()
      .then((config) => {
        if (!cancelled) setBoot({ kind: "ready", config });
      })
      .catch((error: unknown) => {
        if (!cancelled)
          setBoot({ kind: "error", message: error instanceof Error ? error.message : String(error) });
      });
    return () => {
      cancelled = true;
    };
  }, [fleet]);

  if (boot.kind === "loading") return <div className="viewer viewer-message">Connecting…</div>;
  if (boot.kind === "error") {
    return <div className="viewer viewer-message viewer-error">{boot.message}</div>;
  }
  return boot.config.targetWindow ? (
    <SingleWindowStream fleet={fleet} config={boot.config} />
  ) : (
    <MultiWindowStream
      fleet={fleet}
      config={boot.config}
      listWindows={listWindows ?? ((id) => createTeleportBridge().listRemoteWindows(id))}
    />
  );
}

async function currentWindow() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

function SingleWindowStream({ fleet, config }: { fleet: FleetBridge; config: ViewerConfig }) {
  const target = config.targetWindow!;
  const space = config.space;
  const [status, setStatus] = useState<MediaStatus>("connecting");
  const [detail, setDetail] = useState<string | undefined>(undefined);
  const [generation, setGeneration] = useState(0);
  const sessionRef = useRef<MediaSession | null>(null);
  // Anti-feedback: the size we last applied/observed, and a guard while a
  // programmatic resize settles so its `onResized` is not echoed back.
  const applied = useRef({ w: 0, h: 0, guard: false, timer: 0 as number | 0 });
  const replica = target.replica === true;

  const open = useRef<() => Promise<MediaTicket>>(() =>
    fleet
      .openStream(space.id, { kind: "window", windowId: target.id } satisfies StreamTarget, {
        ...(replica ? REPLICA_OPTS : PRIMARY_WINDOW_OPTS),
        geometryControl: !replica,
      })
      .then(toMediaTicket),
  ).current;
  const borrowed: MediaTicket | undefined = target.mediaUrl
    ? { wsUrl: target.mediaUrl, mediaSessionId: target.mediaSessionId }
    : undefined;

  // Resize sync: OS-window resizes → set_window_geometry (debounced, guarded).
  useEffect(() => {
    if (replica) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let timer: number | null = null;
    void (async () => {
      try {
        const win = await currentWindow();
        unlisten = await win.onResized(() => {
          if (disposed || applied.current.guard) return;
          if (timer !== null) window.clearTimeout(timer);
          timer = window.setTimeout(async () => {
            try {
              const scale = await win.scaleFactor();
              const size = await win.innerSize();
              const w = Math.round(size.width / scale);
              const h = Math.round(size.height / scale);
              const last = applied.current;
              if (Math.abs(w - last.w) <= 2 && Math.abs(h - last.h) <= 2) return;
              if (sessionRef.current?.setWindowGeometry(w, h)) {
                applied.current = { ...last, w, h };
              }
            } catch {
              // window went away mid-resize
            }
          }, RESIZE_DEBOUNCE_MS);
        });
      } catch {
        // not inside the Tauri shell
      }
    })();
    return () => {
      disposed = true;
      if (timer !== null) window.clearTimeout(timer);
      unlisten?.();
    };
  }, [replica]);

  const sizeHostTo = (widthPx: number, heightPx: number, scale: number) => {
    const w = Math.round(widthPx / (scale || 1));
    const h = Math.round(heightPx / (scale || 1));
    const state = applied.current;
    if (state.timer) window.clearTimeout(state.timer);
    const timer = window.setTimeout(() => {
      applied.current.guard = false;
    }, RESIZE_DEBOUNCE_MS + 150);
    applied.current = { w, h, guard: true, timer };
    void fleet.resizeStreamWindow(w, h).catch(() => {});
  };

  return (
    <div className="window-stream" data-status={status} data-single="true">
      <div className="window-stream-surface">
        <div className="ws-frame" data-single="true">
          <MediaCanvas
            ticket={borrowed}
            open={borrowed ? undefined : open}
            interactive={!replica}
            audio={!replica}
            generation={generation}
            sessionRef={(s) => {
              sessionRef.current = s;
            }}
            onStatus={(s, d) => {
              setStatus(s);
              setDetail(d);
            }}
            onOpened={(opened) => {
              const scale = opened.geometry?.scale_factor ?? 1;
              if (opened.geometry) sizeHostTo(opened.geometry.width_px, opened.geometry.height_px, scale);
              console.info(`[Cua Spaces] stream window attached to media session ${opened.session_id}`);
              void (async () => {
                try {
                  const win = await currentWindow();
                  // Append rather than replace; strip an id stamped earlier.
                  const base = (await win.title()).split(SESSION_TITLE_SEP)[0];
                  await win.setTitle(`${base}${SESSION_TITLE_SEP}${opened.session_id}`);
                } catch {
                  // not inside the Tauri shell
                }
              })();
            }}
            onGeometry={(w, h) => {
              const scale = sessionRef.current?.sessionOpened?.geometry?.scale_factor ?? 1;
              sizeHostTo(w, h, scale);
            }}
            onGone={() => {
              // The dedicated remote window closed: close this OS window too.
              void currentWindow()
                .then((win) => win.close())
                .catch(() => {});
            }}
          />
          <PresenceOverlay spaceId={space.id} windowId={target.id} interactive={!replica} live={status === "streaming"} />
        </div>
      </div>
      {status !== "streaming" && (
        <div className="window-stream-status">
          {status === "failed" || status === "ended" ? (
            <>
              <span>{detail ?? "The stream ended."}</span>
              {!borrowed && (
                <button type="button" className="ghost-button" onClick={() => setGeneration((g) => g + 1)}>
                  Retry
                </button>
              )}
            </>
          ) : (
            <>
              <span className="window-stream-spinner" aria-hidden="true" />
              <span>{status === "reconnecting" ? "Reconnecting…" : `Connecting to ${target.appName}…`}</span>
              <span className="window-stream-sub">{target.title || space.name}</span>
            </>
          )}
        </div>
      )}
    </div>
  );
}

/** Which windows to stream: the SDK's list (only streamable windows),
 * stable order, capped. */
export function selectWindows(windows: readonly RemoteWindow[], cap = MAX_WINDOW_SESSIONS): RemoteWindow[] {
  return windows.slice(0, cap);
}

function MultiWindowStream({
  fleet,
  config,
  listWindows,
}: {
  fleet: FleetBridge;
  config: ViewerConfig;
  listWindows: (spaceId: string) => Promise<RemoteWindow[]>;
}) {
  const space = config.space;
  const [windows, setWindows] = useState<RemoteWindow[]>([]);
  const [status, setStatus] = useState<"waiting" | "connected">("waiting");

  useEffect(() => {
    let cancelled = false;
    const poll = () => {
      listWindows(space.id)
        .then((list) => {
          if (cancelled) return;
          setStatus("connected");
          setWindows((prev) => {
            const next = selectWindows(list);
            const same =
              prev.length === next.length && prev.every((w, i) => w.id === next[i]?.id && w.title === next[i]?.title);
            return same ? prev : next;
          });
        })
        .catch(() => {
          if (!cancelled) setStatus("waiting");
        });
    };
    poll();
    const timer = window.setInterval(poll, WINDOW_POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [listWindows, space.id]);

  return (
    <div className="window-stream" data-status={status}>
      <div className="window-stream-surface">
        {windows.map((win, index) => (
          <WindowPanel key={win.id} fleet={fleet} spaceId={space.id} win={win} index={index} />
        ))}
      </div>
      {status === "waiting" && (
        <div className="window-stream-status">
          <span className="window-stream-spinner" aria-hidden="true" />
          <span>Waiting for {space.name}…</span>
        </div>
      )}
      {status === "connected" && windows.length === 0 && (
        <div className="window-stream-status">
          <span>No open windows in {space.name} yet.</span>
        </div>
      )}
    </div>
  );
}

function WindowPanel({
  fleet,
  spaceId,
  win,
  index,
}: {
  fleet: FleetBridge;
  spaceId: string;
  win: RemoteWindow;
  index: number;
}) {
  const [pos, setPos] = useState({ x: 30 + ((index * 40) % 300), y: 24 + ((index * 36) % 220) });
  const [title, setTitle] = useState(win.title);
  const [badge, setBadge] = useState("");
  const [closed, setClosed] = useState(false);
  const [z, setZ] = useState(1);
  const sessionRef = useRef<MediaSession | null>(null);
  const open = useRef(() =>
    fleet.openStream(spaceId, { kind: "window", windowId: win.id }, PRIMARY_WINDOW_OPTS).then(toMediaTicket),
  ).current;

  if (closed) return null;

  const startDrag = (event: React.MouseEvent) => {
    if ((event.target as HTMLElement).closest(".ws-frame-close")) return;
    event.preventDefault();
    setZ(2);
    const offsetX = event.clientX - pos.x;
    const offsetY = event.clientY - pos.y;
    const move = (e: MouseEvent) => setPos({ x: e.clientX - offsetX, y: e.clientY - offsetY });
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  return (
    <div className="ws-frame" style={{ left: `${pos.x}px`, top: `${pos.y}px`, zIndex: z }} onMouseDown={() => setZ(2)} onBlur={() => setZ(1)}>
      <div className="ws-frame-bar" onMouseDown={startDrag}>
        <span className="ws-frame-title">{`${win.appName || "Window"}${title ? ` — ${title}` : ""}`}</span>
        <span className="ws-frame-badge">{badge}</span>
        <button
          type="button"
          className="ws-frame-close"
          aria-label="Close window"
          onClick={() => {
            const id = sessionRef.current?.id;
            if (id) void fleet.closeStream(spaceId, id).catch(() => {});
            setClosed(true);
          }}
        >
          ✕
        </button>
      </div>
      <MediaCanvas
        open={open}
        interactive
        audio={false}
        sessionRef={(s) => {
          sessionRef.current = s;
        }}
        onTitle={setTitle}
        onStatus={(s) => setBadge(s === "streaming" ? "" : s)}
        onGone={() => setClosed(true)}
      />
      <PresenceOverlay spaceId={spaceId} windowId={win.id} interactive live={!badge} />
    </div>
  );
}
