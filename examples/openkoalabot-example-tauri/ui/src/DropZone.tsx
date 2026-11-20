// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The one drop zone (`<cua-drop-zone>` from `@trycua/cua/teleport`): files
// land here. The webview's drops are native (Tauri drag-drop events, physical
// pixels), so the zone hit-tests them itself. Its "Teleport an app…" button
// (and a dropped app bundle) only says that app teleport ships with Cua
// Spaces (source-available).
import { useEffect, useRef } from "react"
import { defineDropZoneElement, isInsideZone, type DropZoneStatus } from "@trycua/cua/teleport"

type ZoneElement = HTMLElement & { status: DropZoneStatus; over: boolean }

/** A native drop position (physical pixels) in client coordinates. */
export function dropPoint(position: { x: number; y: number }, scale: number): { x: number; y: number } {
  const s = scale || 1
  return { x: position.x / s, y: position.y / s }
}

/** Whether a native drop at `position` lands on the zone. */
export function dropHits(zone: HTMLElement | null, position: { x: number; y: number }, scale: number): boolean {
  return isInsideZone(zone, dropPoint(position, scale))
}

export function DropZone(p: {
  spaceId: string
  status: DropZoneStatus
  over: boolean
  zoneRef?: (el: HTMLElement | null) => void
  onSendFile: () => void
  onTeleportApp: () => void
}) {
  const ref = useRef<ZoneElement | null>(null)
  if (typeof customElements !== "undefined") defineDropZoneElement()
  const { onSendFile, onTeleportApp } = p
  useEffect(() => {
    const el = ref.current
    if (!el) return
    const send = () => onSendFile()
    const teleport = () => onTeleportApp()
    el.addEventListener("cua-send-file", send)
    el.addEventListener("cua-teleport-app", teleport)
    return () => {
      el.removeEventListener("cua-send-file", send)
      el.removeEventListener("cua-teleport-app", teleport)
    }
  }, [onSendFile, onTeleportApp])
  useEffect(() => {
    if (ref.current) ref.current.status = p.status
  }, [p.status])
  useEffect(() => {
    if (ref.current) ref.current.over = p.over
  }, [p.over])
  return (
    <cua-drop-zone
      ref={(el: HTMLElement | null) => {
        ref.current = el as ZoneElement | null
        if (el) {
          ;(el as ZoneElement).status = p.status
          ;(el as ZoneElement).over = p.over
        }
        p.zoneRef?.(el)
      }}
      data-space={p.spaceId}
    />
  )
}

declare module "react" {
  // eslint-disable-next-line @typescript-eslint/no-namespace
  namespace JSX {
    interface IntrinsicElements {
      "cua-drop-zone": React.DetailedHTMLProps<React.HTMLAttributes<HTMLElement>, HTMLElement> & { "data-space"?: string }
    }
  }
}
