// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { SfIcon } from "./SfIcon";

/**
 * The shared magnifier glyph for the flush filter bars — the `magnifyingglass`
 * SF Symbol on macOS, else the inline SVG fallback below.
 */
export function SearchGlyph({ className }: { className?: string }) {
  return <SfIcon name="magnifyingglass" className={className} fallback={<SearchSvg className={className} />} />;
}

function SearchSvg({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
      <circle cx="7" cy="7" r="4.5" fill="none" stroke="currentColor" strokeWidth="1.3" />
      <path d="m10.5 10.5 3 3" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" />
    </svg>
  );
}
