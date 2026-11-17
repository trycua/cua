// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The sidebar's Space section: the picker, New Space, and Delete. Delete
// is its own control (never inside the pick target) and always asks first:
// deleting a created Space deletes its sandbox; a Space added by address is
// only removed from the list.
import { useState } from "react"
import type { SpaceInfo } from "./api"
import { Plus } from "./icons"

export function SpaceSection(p: {
  spaces: SpaceInfo[]
  space: SpaceInfo | null
  onSpace: (id: string) => void
  onNewSpace: () => void
  onDelete: (id: string) => void
}) {
  const [confirming, setConfirming] = useState<SpaceInfo | null>(null)
  return (
    <div className="space-section">
      <div className="section-label">Space</div>
      <div className="space-box">
        <select className="select" aria-label="Space" value={p.space?.id ?? ""} onChange={(e) => p.onSpace(e.target.value)} disabled={p.spaces.length === 0}>
          {p.spaces.length === 0 && <option value="">No Spaces yet</option>}
          {p.spaces.map((s) => (
            <option key={s.id} value={s.id}>
              {s.name} ({providerLabel(s.provider)})
            </option>
          ))}
        </select>
        <div className="row">
          <button className="btn" onClick={p.onNewSpace}>
            <Plus /> New Space
          </button>
          <button className="btn btn-ghost btn-danger" data-action="delete" disabled={!p.space} onClick={() => p.space && setConfirming(p.space)} title="Delete this Space">
            Delete…
          </button>
        </div>
      </div>
      {confirming && (
        <div className="scrim" role="dialog" aria-modal="true" aria-label="Delete Space">
          <div className="sheet sm">
            <div className="sheet-head">
              <h2>{isCreated(confirming) ? "Delete" : "Remove"} {confirming.name}?</h2>
              <p>
                {confirming.provider === "cloud"
                  ? "This deletes the Cua Cloud Space and everything on it. Metering stops."
                  : confirming.provider === "local"
                    ? "This deletes the Space from this machine, with everything on it."
                    : "This removes the Space from the list. The machine itself keeps running."}
              </p>
            </div>
            <div className="sheet-foot">
              <span className="spacer" />
              <button className="btn" data-action="cancel" onClick={() => setConfirming(null)}>
                Cancel
              </button>
              <button
                className="btn btn-primary"
                data-action="confirm-delete"
                onClick={() => {
                  const id = confirming.id
                  setConfirming(null)
                  p.onDelete(id)
                }}
              >
                {isCreated(confirming) ? "Delete" : "Remove"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}

/** The location a Space runs in, as the app labels it. */
export function providerLabel(provider: string): string {
  return provider === "cloud" ? "Cua Cloud" : provider === "local" ? "This machine" : provider
}

/** A Space this app created (deleting it deletes its sandbox). */
function isCreated(s: SpaceInfo): boolean {
  return s.provider === "cloud" || s.provider === "local"
}
