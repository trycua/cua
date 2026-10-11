// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { SfIcon } from "./SfIcon";

/**
 * Hotspot glyph — the macOS `personalhotspot` SF Symbol on macOS, else the
 * inline broadcast-arcs fallback below. `currentColor` so the caller sets hue.
 */
export function HotspotGlyph({ className }: { className?: string }) {
  return <SfIcon name="personalhotspot" className={className} fallback={<HotspotSvg className={className} />} />;
}

function HotspotSvg({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
      <circle cx="8" cy="12" r="1.4" fill="currentColor" />
      <path
        d="M4.6 9.4a4.8 4.8 0 0 1 6.8 0"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
      />
      <path
        d="M2.4 7.2a8 8 0 0 1 11.2 0"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
      />
    </svg>
  );
}

/** Someone is connected to this machine (the notch's remote-access indicator). */
export function RemoteAccessGlyph({ className }: { className?: string }) {
  return <SfIcon name="person.2.fill" className={className} fallback={<PeopleSvg className={className} />} />;
}

function PeopleSvg({ className }: { className?: string }) {
  return (
    <svg className={className} viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
      <circle cx="5.6" cy="5.4" r="2.2" fill="currentColor" />
      <path d="M1.6 13a4 4 0 0 1 8 0z" fill="currentColor" />
      <circle cx="11.2" cy="6" r="1.8" fill="currentColor" opacity="0.7" />
      <path d="M9.4 13a3.4 3.4 0 0 1 5.4-2.7V13z" fill="currentColor" opacity="0.7" />
    </svg>
  );
}
