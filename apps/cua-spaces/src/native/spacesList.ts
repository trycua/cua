// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { hasTauri } from "./bridge";

/** One Space as the popped-out list window sees it (the portal's roster). */
export interface SpacesListEntry {
  id: string;
  name: string;
  detail: string;
  status: string;
  /** "macos" | "windows" | "linux"; omitted when the provider reports none. */
  os?: string;
}

export interface SpacesListRequest {
  spaces: SpacesListEntry[];
  selectedId?: string | null;
}

/**
 * The main window (label `main`): the Spaces list, the New Space wizard and
 * Settings live there, in an ordinary app window, never in the notch panel.
 * The switcher's list, "+ New" and gear buttons open it through the shell.
 */
export interface SpacesListBridge {
  readonly isNative: boolean;
  /** Open, or refresh + focus, the list window with this roster. */
  open(request: SpacesListRequest): Promise<void>;
  /** Read this window's roster (only valid on the list window). */
  config(): Promise<SpacesListRequest>;
  /** Close the list window. */
  close(): Promise<void>;
  /** Open the main window on its New Space wizard. */
  openNewSpace(): Promise<void>;
  /** Open the main window on Settings. */
  openSettings(): Promise<void>;
}

export function createFallbackSpacesListBridge(): SpacesListBridge {
  return {
    isNative: false,
    open: async () => {},
    config: async () => {
      throw new Error("the Spaces list window needs the Tauri shell");
    },
    close: async () => {},
    openNewSpace: async () => {},
    openSettings: async () => {},
  };
}

export function createTauriSpacesListBridge(): SpacesListBridge {
  const core = import("@tauri-apps/api/core");
  const invoke = async <T>(command: string, args?: Record<string, unknown>) =>
    (await core).invoke<T>(command, args);
  return {
    isNative: true,
    open: (request) => invoke<void>("open_spaces_list", { request }),
    config: () => invoke<SpacesListRequest>("spaces_list_config"),
    close: () => invoke<void>("close_spaces_list"),
    openNewSpace: () => invoke<void>("open_new_space"),
    openSettings: () => invoke<void>("open_main_settings"),
  };
}

export function createSpacesListBridge(): SpacesListBridge {
  return hasTauri() ? createTauriSpacesListBridge() : createFallbackSpacesListBridge();
}
