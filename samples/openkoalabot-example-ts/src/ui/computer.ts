// The Computer panel's pieces under the stream: the Space's window list, one
// line per window (the app's icon from the Space, when it has one, then the
// name) with a picture-in-picture button, and the one drop zone
// (`<cua-drop-zone>` from `@trycua/cua/teleport`) for files. Its "Teleport an
// app…" button and app drops show that app teleport ships with Cua Spaces.
import { pipLabel, windowSource, type PipSource } from "@trycua/cua/spaces/pip"
import type { DropZoneStatus } from "@trycua/cua/teleport"
import { h, icon } from "./dom.js"

/** A row of `GET /api/windows`. */
export interface WindowRow {
  windowId: string
  app: string
  title: string
  width?: number
  height?: number
  /** The app icon the Space's desktop shows (`data:` URL), when it has one. */
  icon?: string
}

export interface WindowListProps {
  windows: WindowRow[]
  /** Whether this source is in the PiP now. */
  isOpen: (source: PipSource) => boolean
  /** False when the browser has no picture-in-picture. */
  canPip: boolean
  onPip: (window: WindowRow) => void
}

/** The window list, or null when the Space lists no windows. */
export function renderWindowList(p: WindowListProps): HTMLElement | null {
  if (!p.windows.length) return null
  return h(
    "div",
    { class: "win-list", role: "list", "aria-label": "Windows" },
    ...p.windows.map((w) => {
      const src = windowSource(w)
      const on = p.isOpen(src)
      return h(
        "div",
        { class: "win-row", role: "listitem", "data-window-id": w.windowId },
        // The Space's own icon, or nothing: never a stand-in glyph.
        w.icon ? h("img", { class: "win-icon", src: w.icon, alt: "", "aria-hidden": "true" }) : null,
        h("span", { class: "win-name", title: pipLabel(src) }, pipLabel(src)),
        h(
          "button",
          {
            class: `icon-btn sm ${on ? "on" : ""}`,
            title: on ? "Close picture in picture" : "Picture in picture",
            "aria-pressed": on ? "true" : "false",
            disabled: !p.canPip,
            onclick: () => p.onPip(w),
          },
          icon("pip", 14),
        ),
      )
    }),
  )
}

export interface DropZoneProps {
  status: DropZoneStatus
  /** A DOM drop: files, or an app bundle (the host tells them apart). */
  onDrop: (dt: DataTransfer) => void
  onSendFile: () => void
  /** "Teleport an app…" (the sample says it ships with Cua Spaces). */
  onTeleportApp: () => void
}

/** The one drop zone. */
export function renderDropZone(p: DropZoneProps): HTMLElement {
  const zone = document.createElement("cua-drop-zone") as HTMLElement & { status: DropZoneStatus }
  zone.status = p.status
  zone.addEventListener("cua-send-file", () => p.onSendFile())
  zone.addEventListener("cua-teleport-app", () => p.onTeleportApp())
  zone.addEventListener("cua-drop", (e) => p.onDrop((e as CustomEvent<DataTransfer>).detail))
  return zone
}
