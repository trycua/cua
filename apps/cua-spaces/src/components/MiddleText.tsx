// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/** The tail kept whole when a path's last part is long. */
const TAIL = 18;

/**
 * A path on one line that shortens in the middle, never at the end: the
 * start fades to an ellipsis while its last part stays whole. `title` is
 * the tooltip (the full path). The text reads whole to assistive
 * technology.
 */
export function MiddleText({ text, title, className }: { text: string; title?: string | null; className?: string }) {
  const slash = text.lastIndexOf("/");
  let cut = slash > 0 ? slash : text.length;
  if (text.length - cut > TAIL) cut = Math.max(0, text.length - TAIL);
  const head = text.slice(0, cut);
  const tail = text.slice(cut);
  return (
    <span className={className ? `mid-text ${className}` : "mid-text"} title={title ?? undefined} aria-label={text}>
      <span className="mid-text-head" aria-hidden="true">
        {head}
      </span>
      <span className="mid-text-tail" aria-hidden="true">
        {tail}
      </span>
    </span>
  );
}
