// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// Picture in picture: a small always-on-top window per source (the desktop,
// or one window of the Space). The shell opens it (`open_pip`); its page is
// this app with a `#pip=…` route (`@trycua/cua/spaces/pip`), which renders
// only <PipView>: its own media session on that source, closed with it.
import { useCallback, useEffect, useRef, useState } from "react"
import { listen } from "@tauri-apps/api/event"
import {
  desktopSource,
  encodePipRoute,
  parsePipRoute,
  pipLabel,
  pipSize,
  pipWindowLabel,
  windowSource,
  type PipSource,
} from "@trycua/cua/spaces/pip"
import { api, type WindowRow } from "./api"
import * as I from "./icons"
import { attach, type StreamStatus } from "./rcdp"

export { desktopSource }

/** The route of a PiP window: the source plus the Space it belongs to. */
export function pipRoute(source: PipSource, space: string): string {
  return `${encodePipRoute(source)}&space=${encodeURIComponent(space)}`
}

/** What a PiP page shows, or null when this page is the main window. */
export function readPipRoute(hash: string): { source: PipSource; space: string } | null {
  const source = parsePipRoute(hash)
  const space = new URLSearchParams(hash.replace(/^#/, "")).get("space")
  return source && space ? { source, space } : null
}

/** The size a PiP opens at: the source's aspect, 480 points on the long edge. */
export function pipOpenSize(source: PipSource, desktop: { width: number; height: number }, window?: { width: number; height: number }) {
  const s = source.kind === "window" && window ? window : desktop
  return pipSize(s.width, s.height, 480)
}

/** Which PiPs are open, keyed by window label; `pip-closed` keeps it true. */
export function usePip(space: string | null) {
  const [open, setOpen] = useState<Set<string>>(new Set())
  useEffect(() => {
    const un = listen<string>("pip-closed", ({ payload }) =>
      setOpen((s) => {
        const n = new Set(s)
        n.delete(payload)
        return n
      }),
    )
    return () => {
      un.then((f) => f())
    }
  }, [])
  // A PiP belongs to its Space: switching Spaces closes them.
  const openRef = useRef(open)
  openRef.current = open
  useEffect(
    () => () => {
      for (const label of openRef.current) void api.closePip(label).catch(() => {})
      setOpen(new Set())
    },
    [space],
  )
  const isOpen = useCallback((source: PipSource) => open.has(pipWindowLabel(source)), [open])
  const toggle = useCallback(
    async (source: PipSource, size: { width: number; height: number }, spaceName: string) => {
      if (!space) return
      const label = pipWindowLabel(source)
      if (open.has(label)) {
        await api.closePip(label)
        setOpen((s) => {
          const n = new Set(s)
          n.delete(label)
          return n
        })
        return
      }
      await api.openPip(label, pipRoute(source, space), pipLabel(source, spaceName), size.width, size.height)
      setOpen((s) => new Set(s).add(label))
    },
    [open, space],
  )
  return { isOpen, toggle }
}

/** The whole page of a PiP window: the stream, edge to edge. */
export function PipView({ source, space }: { source: PipSource; space: string }) {
  const canvas = useRef<HTMLCanvasElement>(null)
  const [status, setStatus] = useState<StreamStatus | null>(null)
  const [error, setError] = useState("")
  useEffect(() => {
    let close: (() => void) | undefined
    let cancelled = false
    api
      .openStream(space, source.kind === "window" ? source.windowId : undefined)
      .then((t) => {
        if (!cancelled && canvas.current) close = attach(t.ws_url, canvas.current, setStatus)
      })
      .catch((e) => setError(String(e)))
    return () => {
      cancelled = true
      close?.()
    }
  }, [source, space])
  // No traffic lights (like macOS's own PiP): Escape or the close button
  // that appears on hover closes the window.
  const close = useCallback(() => void api.closePip(pipWindowLabel(source)).catch(() => {}), [source])
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close()
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [close])
  const live = !!status && status.frames > 0
  return (
    <div className="pip-view" data-tauri-drag-region>
      <canvas ref={canvas} data-frames={status?.frames ?? 0} />
      <button className="pip-close" title="Close" aria-label="Close picture in picture" onClick={close}>
        <I.Close />
      </button>
      {!live && <div className="pip-status">{error || (status ? `Stream ${status.state}` : "Connecting…")}</div>}
    </div>
  )
}

/** The Space's windows: one line each, with a picture-in-picture button. */
export function WindowList({ windows, isOpen, onPip }: { windows: WindowRow[]; isOpen: (s: PipSource) => boolean; onPip: (w: WindowRow) => void }) {
  if (!windows.length) return null
  return (
    <div className="win-list" role="list" aria-label="Windows">
      {windows.map((w) => {
        const src = windowSource(w)
        const on = isOpen(src)
        return (
          <div className="win-row" role="listitem" key={w.window_id}>
            {w.icon && <img className="win-icon" src={w.icon} alt="" aria-hidden="true" />}
            <span className="win-name" title={pipLabel(src)}>
              {pipLabel(src)}
            </span>
            <button className={`icon-btn sm ${on ? "on" : ""}`} title={on ? "Close picture in picture" : "Picture in picture"} aria-pressed={on} onClick={() => onPip(w)}>
              <I.Pip />
            </button>
          </div>
        )
      })}
    </div>
  )
}
