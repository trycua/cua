// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The Teleport drop zone: the dashed region beneath the Windows section on the
 * pop-out list window's second page (a Space's targets).
 *
 * Two things can be teleported into it, and they are NOT the same operation:
 *
 *   * a FILE — copied to the Space's `~/Downloads`, verified by digest there
 *     before this reports it landed (`send_files_to_space`); and
 *   * an APP — the existing logged-in-session teleport, which is the picker flow
 *     the page-2 footer already opens. This zone calls straight into that same
 *     path (`onTeleportApp`) rather than growing a second teleport.
 *
 * Both accept a drop as well as a click, including a dragged window "the same
 * way you can into the notch": the notch tiles hit-test the global AX
 * window-drag against their own bounds and mark themselves
 * `data-drop-target="true"`, and so does this.
 *
 * Page 1 has no selected Space, so the zone is only ever rendered on page 2 —
 * the same rule the footer actions follow.
 */
import { useCallback, useEffect, useRef, useState } from "react";

import { handleViewerAppDrop } from "../native/appDrop";
import { core } from "../core";
import { isResize } from "../model/dragTrigger";
import { detailCopy } from "../model/window";
import { Sym } from "./desktop/Sym";
import type { FileSendBridge, SentFile } from "../native/fileSend";
import { screenToClient, type WindowDragBridge } from "../native/windowDrag";

export interface TeleportDropZoneProps {
  spaceId: string;
  spaceName: string;
  fileSend: FileSendBridge;
  /** Opens the existing app-teleport picker for this Space (footer's action). */
  onTeleportApp: () => void;
  /** Global window-drag monitor, so a dragged window can be dropped in here. */
  windowDrag?: WindowDragBridge;
  /**
   * An app bundle dropped here (Finder, Dock) opens "Teleport an app…" for
   * it; resolves true when the drop was an app. Default: the shell picker.
   */
  appDrop?: (paths: string[]) => Promise<boolean>;
}

type Status =
  | { kind: "idle" }
  | { kind: "sending"; paths: string[] }
  | { kind: "sent"; files: SentFile[] }
  | { kind: "failed"; message: string };

/** What the zone says it did (the app core's words). Deliberately concrete:
 * a vague "Sent" is how a lost file passes for a delivered one. */
function statusLine(status: Status): string | null {
  switch (status.kind) {
    case "idle":
      return null;
    case "sending":
      return core<string>("transfer.dropSendingText", { paths: status.paths });
    case "sent": {
      const line = core<string>("transfer.dropSentText", {
        files: status.files.map((f) => ({ name: f.name, dest: f.dest, bytes: f.bytes })),
      });
      return line || null;
    }
    case "failed":
      return status.message;
  }
}

export default function TeleportDropZone({
  spaceId,
  spaceName,
  fileSend,
  onTeleportApp,
  windowDrag,
  appDrop,
}: TeleportDropZoneProps) {
  const copy = detailCopy();
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const [over, setOver] = useState(false);
  const zoneRef = useRef<HTMLDivElement | null>(null);
  const busy = status.kind === "sending";
  const busyRef = useRef(busy);
  busyRef.current = busy;

  const send = useCallback(
    async (paths: string[]) => {
      if (!paths.length || busyRef.current) return;
      setStatus({ kind: "sending", paths });
      try {
        const files = await fileSend.sendFiles(spaceId, paths);
        setStatus({ kind: "sent", files });
      } catch (error) {
        setStatus({ kind: "failed", message: error instanceof Error ? error.message : String(error) });
      }
    },
    [fileSend, spaceId],
  );

  const selectFile = useCallback(async () => {
    try {
      const paths = await fileSend.pickFiles();
      await send(paths);
    } catch (error) {
      setStatus({ kind: "failed", message: error instanceof Error ? error.message : String(error) });
    }
  }, [fileSend, send]);

  // -- dropping a FILE from Finder (Tauri's native webview drag-drop) ---------
  useEffect(() => {
    if (!fileSend.isNative) return;
    let dispose: (() => void) | null = null;
    let cancelled = false;
    void (async () => {
      try {
        const { getCurrentWebview } = await import("@tauri-apps/api/webview");
        const unlisten = await getCurrentWebview().onDragDropEvent((event) => {
          const payload = event.payload;
          if (payload.type === "leave") {
            setOver(false);
            return;
          }
          if (payload.type !== "over" && payload.type !== "drop") return;
          // Drag positions are PHYSICAL pixels; the DOM hit-test is logical.
          const scale = window.devicePixelRatio || 1;
          const inside = isInsideZone(zoneRef.current, {
            x: payload.position.x / scale,
            y: payload.position.y / scale,
          });
          if (payload.type === "over") {
            setOver(inside);
            return;
          }
          setOver(false);
          if (inside && payload.paths?.length) {
            const paths = payload.paths;
            const route = appDrop ?? ((p: string[]) => handleViewerAppDrop(p, { id: spaceId, name: spaceName }));
            void route(paths).then((wasApp) => {
              if (!wasApp) void send(paths);
            });
          }
        });
        if (cancelled) unlisten();
        else dispose = unlisten;
      } catch {
        // No webview drag-drop here; the buttons still work.
      }
    })();
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [fileSend.isNative, send, appDrop, spaceId, spaceName]);

  // -- dropping a WINDOW, the way the notch tiles accept one -----------------
  // The AX monitor reports the drag in screen points, so the zone converts them
  // into this window's client space before hit-testing itself.
  useEffect(() => {
    if (!windowDrag?.isNative) return;
    let dispose: (() => void) | null = null;
    let cancelled = false;
    let origin: { x: number; y: number } | null = null;
    void (async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        const refreshOrigin = async () => {
          const [position, scale] = await Promise.all([win.innerPosition(), win.scaleFactor()]);
          origin = { x: position.x / scale, y: position.y / scale };
        };
        await refreshOrigin();
        // A window resized from an edge or a corner is not dropped anywhere
        // (the app core compares its frames).
        let resizing = false;
        const unlisten = await windowDrag.onWindowDrag((drag) => {
          if (drag.phase === "start") {
            resizing = isResize(drag);
            void refreshOrigin();
          }
          if (!origin || resizing) return;
          const inside = isInsideZone(zoneRef.current, screenToClient(drag, origin));
          if (drag.phase === "end") {
            setOver(false);
            // A window carries a logged-in app session, so it takes the app
            // teleport path — the same one the footer button opens.
            if (inside) onTeleportApp();
            return;
          }
          setOver(inside);
        });
        if (cancelled) unlisten();
        else dispose = unlisten;
      } catch {
        // No window-drag monitor (no Accessibility permission): clicks still work.
      }
    })();
    return () => {
      cancelled = true;
      dispose?.();
    };
  }, [windowDrag, onTeleportApp]);

  const line = statusLine(status);
  return (
    <section className="swl-section sl-teleport" aria-label={`Teleport to ${spaceName}`}>
      {/* No label here: the page's section title names it (one label per thing). */}
      <div
        ref={zoneRef}
        className="sl-dropzone"
        data-drop-target={over ? "true" : undefined}
        data-busy={busy ? "true" : undefined}
      >
        <Sym name={over ? copy.teleportSymbolActive : copy.teleportSymbol} size={22} className="sl-dropzone-icon" />
        <p className="sl-dropzone-caption">{copy.dropCaption}</p>
        <div className="sl-dropzone-actions">
          <button type="button" className="sl-dropzone-button" onClick={selectFile} disabled={busy}>
            {copy.sendFile}
          </button>
          <button type="button" className="sl-dropzone-button" onClick={onTeleportApp} disabled={busy}>
            {copy.teleportApp}
          </button>
        </div>
        {line ? (
          <p className="sl-dropzone-status" data-kind={status.kind} role="status">
            {line}
          </p>
        ) : null}
      </div>
    </section>
  );
}

/** Whether a client-space point falls inside the zone's box. Exported so the
 * drop hit-test is testable without a real drag. */
export function isInsideZone(
  element: HTMLElement | null,
  point: { x: number; y: number },
): boolean {
  if (!element) return false;
  const box = element.getBoundingClientRect();
  if (box.width === 0 && box.height === 0) return false;
  return (
    point.x >= box.left && point.x <= box.right && point.y >= box.top && point.y <= box.bottom
  );
}
