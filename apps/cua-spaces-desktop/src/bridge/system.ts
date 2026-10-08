// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// What the bridge asks of the operating system beyond the windows (the
// SwiftUI host's NSWorkspace, UNUserNotificationCenter, LocalAuthentication,
// SMAppService and Sparkle calls): opening a page or a settings pane,
// showing a file, posting a notification, the owner check, the login item
// and the updater. `src/system.ts` answers them with Electron; tests answer
// them in memory, so no test touches the real login items, notifications
// or keychain.
import type { LoginItemControlling } from "../model/login-item";
import type { UpdaterDriving } from "../model/updates";

/** A system notification; clicking it brings the app to `route`. */
export interface SystemNote {
  /** Replaces an earlier note with the same id. */
  id: string;
  title: string;
  body: string;
  route?: string;
}

export interface SystemServices {
  /** Opens a web page, or a system settings pane (`x-apple.systempreferences:`, `ms-settings:`). */
  openExternal(url: string): Promise<void>;
  /** Shows a file or folder in Finder, Explorer or the file manager. */
  reveal(path: string): void;
  /** Posts a notification (nothing when the system does not show them). */
  notify(note: SystemNote): void;
  /** The owner check before an account change only the person here may make; throws when it was not confirmed. */
  confirmPresence(reason: string): Promise<void>;
  /** Launch at login (null: this build has none, such as a development run). */
  loginItem: LoginItemControlling | null;
  /** Settings → About's updater (null: updates are off in this build). */
  updater: UpdaterDriving | null;
  /** The user's home folder, as the daemon sees it. */
  home: string;
}
