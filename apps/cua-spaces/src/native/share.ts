// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import type { ShareEntryInput } from '../model/share';
import { hasTauri } from './bridge';

/** The `space_shares` / `share_space` / `unshare_space` result. */
export interface SpaceSharesResult {
  space: string;
  machine: string;
  invitee_space: string;
  url: string;
  online: boolean;
  shares: ShareEntryInput[];
}

/**
 * Sharing a Space through the account's relay (the shell's sharing
 * commands over the app's Spaces runtime). Sharing asks for presence in the
 * shell first on macOS.
 */
export interface ShareBridge {
  shares(spaceId: string): Promise<SpaceSharesResult>;
  share(spaceId: string, who: string, role: string): Promise<SpaceSharesResult>;
  unshare(spaceId: string, who: string): Promise<SpaceSharesResult>;
}

export function createTauriShareBridge(): ShareBridge {
  const core = import('@tauri-apps/api/core');
  const invoke = async <T>(command: string, args?: Record<string, unknown>) =>
    (await core).invoke<T>(command, args);
  return {
    shares: (spaceId) => invoke('space_shares', { spaceId }),
    share: (spaceId, who, role) => invoke('share_space', { spaceId, who, role }),
    unshare: (spaceId, who) => invoke('unshare_space', { spaceId, who }),
  };
}

export function createShareBridge(): ShareBridge {
  if (hasTauri()) return createTauriShareBridge();
  const unavailable = () => Promise.reject(new Error('Sharing needs the Cua Spaces app'));
  return { shares: unavailable, share: unavailable, unshare: unavailable };
}

/** In-memory relay for tests: `presenceFails` refuses as a cancelled Touch ID would. */
export function createFakeShareBridge(
  options: { shares?: ShareEntryInput[]; presenceFails?: string } = {}
): ShareBridge & { calls: string[] } {
  let shares = [...(options.shares ?? [])];
  const calls: string[] = [];
  const result = (space: string): SpaceSharesResult => ({
    space,
    machine: 'space-00000000000000aa',
    invitee_space: 'relay:space-00000000000000aa',
    url: '',
    online: true,
    shares,
  });
  return {
    calls,
    shares: async (space) => {
      calls.push(`shares:${space}`);
      return result(space);
    },
    share: async (space, who, role) => {
      calls.push(`share:${space}:${who}:${role}`);
      if (options.presenceFails) throw new Error(options.presenceFails);
      shares = [...shares.filter((s) => s.who !== who), { who, role, connected: false }];
      return result(space);
    },
    unshare: async (space, who) => {
      calls.push(`unshare:${space}:${who}`);
      shares = shares.filter((s) => s.who !== who);
      return result(space);
    },
  };
}
