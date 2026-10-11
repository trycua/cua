// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { osIcon, osIconUrl } from "../model/notch";
import type { Space } from "../model/types";
import { SfIcon } from "./SfIcon";

/**
 * The Space's OS icon (the core's per-tile id): the Linux distribution's
 * mark when the Space reported one, else macOS, Windows or Linux. macOS uses
 * the system `apple.logo` symbol where it can render; everything else is the
 * core's single-color SVG, tinted with `currentColor`.
 */
export function OsIcon({ space, className }: { space: Pick<Space, "os" | "osName">; className?: string }) {
  return <OsIconMark id={osIcon(space.os, space.osName)} className={className} />;
}

/** An OS icon by the core's id (`os-ubuntu`, `os-macos`, ...). */
export function OsIconMark({ id, className, size = 11 }: { id: string; className?: string; size?: number }) {
  const url = osIconUrl(id);
  const cls = className ? `sf-icon os-icon ${className}` : "sf-icon os-icon";
  const art = url ? (
    <span
      className={cls}
      data-os-icon={id}
      style={{ maskImage: `url("${url}")`, WebkitMaskImage: `url("${url}")` }}
      aria-hidden="true"
    />
  ) : null;
  if (id === "os-macos") return <SfIcon name="apple.logo" size={size} className="os-icon" fallback={art} />;
  return art;
}
