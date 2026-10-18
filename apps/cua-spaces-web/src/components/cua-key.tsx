// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cn } from "@/lib/utils";

/**
 * The Cua key: the app icon's blue keycap with the official Cua mark, on its
 * own (rendered by apps/cua-spaces-macos/scripts/dmg-art/key.html). It is the
 * icon of Cua's own notifications, so they read as Cua's in light and dark
 * without a colored tile. It says who, never what: unread and error stay with
 * their own dot and color. `size` is in CSS px (16 to 40); the browser picks
 * the sharpest of the 32, 64 and 128 px files for the display.
 */
export function CuaKey({ size = 20, className, label }: { size?: number; className?: string; label?: string }) {
  return (
    <img
      src="/cua-key-64.png"
      srcSet="/cua-key-32.png 32w, /cua-key-64.png 64w, /cua-key-128.png 128w"
      sizes={`${size}px`}
      width={size}
      height={size}
      alt={label ?? ""}
      aria-hidden={label ? undefined : true}
      draggable={false}
      className={cn("shrink-0 select-none object-contain", className)}
    />
  );
}
