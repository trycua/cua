// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import * as path from "node:path";

// The window icon on Linux. X11 panels and docks read _NET_WM_ICON from the
// window, which Electron only sets when BrowserWindow gets an `icon`; without
// it XFCE's task list shows a generic window. Windows takes the icon from the
// exe and macOS from the bundle, so they get none.
export const LINUX_WINDOW_ICON = "128x128@2x.png";

export function windowIconPath(o: {
  platform: NodeJS.Platform;
  packaged: boolean;
  resourcesPath: string;
  devIconsDir: string;
  exists: (file: string) => boolean;
}): string | undefined {
  if (o.platform !== "linux") return undefined;
  const file = o.packaged ? path.join(o.resourcesPath, "tray", LINUX_WINDOW_ICON) : path.join(o.devIconsDir, LINUX_WINDOW_ICON);
  return o.exists(file) ? file : undefined;
}
