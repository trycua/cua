// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The agent's activity in a thread: muted one-line steps, folded under a
// one-line summary (the SDK's `activity` transcript items).
import { useEffect, useState } from "react"

/** A run of the agent's non-message output (install, tools, thinking, turn
 *  ends, notices): muted one-line rows, folded under a one-line summary. */
export function ActivityGroup({ summary, steps, defaultOpen = false }: { summary: string; steps: string[]; defaultOpen?: boolean }) {
  const [open, setOpen] = useState(defaultOpen)
  // A group grows as steps arrive; one that comes to match opens then.
  useEffect(() => {
    if (defaultOpen) setOpen(true)
  }, [defaultOpen])
  return (
    <div className={`activity ${open ? "open" : ""}`}>
      <button className="activity-head" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span className="activity-chevron" aria-hidden="true">
          {"\u203A"}
        </span>
        {summary}
      </button>
      {open && (
        <div className="activity-steps" role="list">
          {steps.map((s, i) => (
            <div key={i} className="activity-step" role="listitem" title={s}>
              {s}
            </div>
          ))}
        </div>
      )}
    </div>
  )
}
