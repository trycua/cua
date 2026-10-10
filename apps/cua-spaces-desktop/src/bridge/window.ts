// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// The window the page is in (`window.*`): its background as the page's
// appearance changes, and its drag regions. Chromium reads where the window
// drags from the page's CSS (`app-region`), so the Electron page never sends
// them (the webkit adapter skips it here); the rects are checked and the
// CSS stays in charge.
import type { BridgeContext } from "./context";
import { Failure, type Handlers } from "./host";

const APPEARANCES = ["system", "light", "dark"] as const;

export const isRect = (r: unknown) =>
  !!r && typeof r === "object" && ["x", "y", "width", "height"].every((k) => typeof (r as Record<string, unknown>)[k] === "number");

export function windowMethods({ ui }: BridgeContext): Handlers {
  return {
    "window.setBackgroundColor": (args, caller) => {
      const color = typeof args.color === "string" ? args.color.trim() : "";
      const hex = /^#?([0-9a-f]{6})$/i.exec(color)?.[1];
      if (!hex) throw Failure.badArgs("color: #rrggbb");
      const appearance = APPEARANCES.find((a) => a === args.appearance) ?? null;
      ui.setBackground(caller.window, `#${hex.toLowerCase()}`, appearance);
      return null;
    },
    "window.setDragRegions": (args) => {
      if (!Array.isArray(args.rects) || !args.rects.every(isRect)) throw Failure.badArgs("rects: [{x,y,width,height}]");
      return null;
    },
  };
}
