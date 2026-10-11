// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { WindowMode } from "../native/types";

/**
 * Black cap that sits over the physical notch so the shell reads as one
 * continuous shape with the hardware. Purely visual.
 *
 * `teleport` is set while a drag teleport is morphing the notch: it instantly
 * rounds the cap's bottom-right corner (which is normally squared in ambient to
 * meet the "N Spaces" tab) so no sharp corner shows before the taller rounded
 * "Teleport to Cua" box slides in over it.
 */
export function NotchCap({ mode, teleport = false }: { mode: WindowMode; teleport?: boolean }) {
  return (
    <div className="notch-cap" data-mode={mode} data-teleport={teleport ? "on" : "off"} aria-hidden="true" />
  );
}
