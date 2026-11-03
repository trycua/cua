// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { MenuItem } from '../model/window';
import { hasTauri } from './bridge';

/** Shows the core's menu in the menu bar item (`tray_set_menu`). */
export type SetTrayMenu = (items: MenuItem[]) => Promise<void>;

export function createSetTrayMenu(): SetTrayMenu {
  if (hasTauri()) {
    const core = import('@tauri-apps/api/core');
    return async (items) => (await core).invoke<void>('tray_set_menu', { items });
  }
  // No menu bar item outside the shell.
  return async () => {};
}
