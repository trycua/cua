// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

/**
 * The menu bar item's menu, from the notch panel: the app core's
 * `window.menu` of the same roster the notch tab counts (with "This
 * machine") and Cua Volume's sync, sent to the shell after each roster
 * change and each sync read, so the menu and the notch say the same number.
 */
import { useEffect, useRef, useState } from 'react';

import type { DriveStorageInput } from '../model/driveSettings';
import type { DriveSyncInput } from '../model/persistent';
import type { Space } from '../model/types';
import { trayMenu } from '../model/window';
import { answerOrNull } from '../native/drive';
import type { ToolCall } from '../native/persistent';
import type { SetTrayMenu } from '../native/tray';
import { useExperiments } from './experiments';

/** How often the menu reads Cua Volume's sync status. */
export const TRAY_SYNC_POLL_MS = 5000;

export function useTrayMenu(
  spaces: readonly Space[],
  {
    call,
    setMenu,
    keyvault = null,
    now = Date.now,
  }: { call: ToolCall; setMenu: SetTrayMenu; keyvault?: string | null; now?: () => number },
): void {
  // The sync read, the store's backend (this Mac's store with one device
  // has nothing to sync) and when they were read: each read recomputes the
  // menu, so a bucket that stopped answering goes Offline on time.
  const [{ sync, backend, nowMs }, setRead] = useState<{
    sync: DriveSyncInput | null;
    backend: string | null;
    nowMs: number;
  }>(() => ({ sync: null, backend: null, nowMs: now() }));
  useEffect(() => {
    let stopped = false;
    const read = () =>
      void Promise.all([
        answerOrNull<DriveSyncInput>(call, 'volume_sync_status'),
        answerOrNull<DriveStorageInput>(call, 'volume_storage'),
      ]).then(([next, storage]) => {
        if (!stopped) setRead({ sync: next, backend: storage?.backend ?? null, nowMs: now() });
      });
    read();
    const id = setInterval(read, TRAY_SYNC_POLL_MS);
    return () => {
      stopped = true;
      clearInterval(id);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [call]);

  // Cua Volume's sync line and conflicts only with its experiment on.
  const experiments = useExperiments();
  const sent = useRef<string | null>(null);
  useEffect(() => {
    const items = trayMenu({ spaces: [...spaces], keyvault, sync, backend, nowMs, experiments });
    const key = JSON.stringify(items);
    if (key === sent.current) return;
    sent.current = key;
    void setMenu(items).catch(() => {
      sent.current = null;
    });
  }, [spaces, keyvault, sync, backend, nowMs, experiments, setMenu]);
}
