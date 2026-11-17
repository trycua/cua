// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { DisplayStyle, DisplayStyleSource } from "../native/types";

interface DevDisplayToggleProps {
  style: DisplayStyle;
  source: DisplayStyleSource;
  onChange: (style: DisplayStyle) => void;
}

/**
 * Development-only switch between notched and no-notch presentation.
 * Rendered only when `import.meta.env.DEV` is true (see App.tsx).
 */
export function DevDisplayToggle({ style, source, onChange }: DevDisplayToggleProps) {
  const next: DisplayStyle = style === "notched" ? "no-notch" : "notched";
  return (
    <button
      type="button"
      className="dev-toggle"
      data-owns-enter
      onClick={() => onChange(next)}
      title={`Display style: ${style} (${source}). Click or press ⌘⇧D to preview ${next}.`}
      aria-label={`Development display toggle. Currently ${style}. Switch to ${next}.`}
    >
      <span className="dev-toggle-dot" aria-hidden="true" />
      {style === "notched" ? "Notch" : "No notch"}
    </button>
  );
}
