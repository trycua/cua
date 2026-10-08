// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

// What every area of the bridge works with: the app's model (the SwiftUI
// app's `AppModel`), the events it pushes, and what it asks of the
// windows (the SwiftUI host's `WebUIActions` and window controller).
import type { KvCredentialForm, KvDeleteConfirm, KvUnlockPrompt } from "../native/generated/index";
import type { AppModel } from "../model/app-model";
import type { DaemonSupervisor } from "../model/daemon";
import type { PipPresenter } from "../model/streams";
import type { CredentialAnswer, UnlockAnswer } from "../model/keyvault";
import type { TeleportCache } from "../model/teleport";
import type { BridgeEvents } from "./host";
import type { SystemServices } from "./system";

/** What the bridge asks the shell's windows to do (Electron owns them). */
export interface BridgeUi {
  /** A Space's desktop in a window of its own. */
  openSpace(spaceId: string, name: string): void;
  /** The calling window's background (and its chrome's appearance). */
  setBackground(window: unknown, color: string, appearance: "system" | "light" | "dark" | null): void;
  /** Brings the app to the front (a keychain prompt shows over the app that asked). */
  activate(): void;
  /** "Send file…": the native file picker over the calling window (files and folders, several); null when cancelled. */
  chooseFiles?(window: unknown): Promise<string[] | null>;
  /** A Space's picture-in-picture panels (floating windows). */
  pip?: PipPresenter;
  /**
   * The Keyvault's prompts, native to this app (the SwiftUI host's
   * `WebUIWindowController.ask` sheets and form). None given: an unlock is
   * denied, a delete declined and a passphrase form is `native_only`.
   */
  /** The unlock prompt (Allow, Deny, Never ask again) over the calling window. */
  askUnlock?(window: unknown, prompt: KvUnlockPrompt): Promise<UnlockAnswer>;
  /** The delete confirmation over the calling window. */
  askDelete?(window: unknown, confirm: KvDeleteConfirm): Promise<boolean>;
  /** The passphrase form (setup or unlock): a window of this app, never the page, so a passphrase does not cross the bridge. Null when cancelled. */
  askPassphrase?(window: unknown, form: KvCredentialForm): Promise<CredentialAnswer>;
}

export interface BridgeContext {
  model: AppModel;
  events: BridgeEvents;
  ui: BridgeUi;
  /** This app's daemon (null in a build without the bundled `cua`). */
  supervisor: DaemonSupervisor | null;
  /** The app's version (`app.info`). */
  version: string;
  platform: NodeJS.Platform;
  /** Every method the registry routes, in order (`app.info`). */
  methods: () => string[];
  env: NodeJS.ProcessEnv;
  /** What Teleport keeps between the page's steps (a new one when omitted; the tests look at it). */
  teleportCache?: TeleportCache;
  /** The system's side: links and settings panes, notifications, the owner check, the login item, the updater (none in tests that leave it out). */
  system?: SystemServices;
}
