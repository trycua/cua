// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { describe, expect, it } from 'vitest';

import tauriConf from '../../src-tauri/tauri.conf.json';

// The shipped updater config: signed updates from the public release feed.
const conf = tauriConf as unknown as {
  plugins: { updater: { endpoints: string[]; pubkey: string } };
};

describe('auto-update feed', () => {
  const updater = conf.plugins.updater;

  it('reads only the public trycua/cua release feed over HTTPS', () => {
    expect(updater.endpoints.length).toBeGreaterThan(0);
    for (const endpoint of updater.endpoints) {
      const url = new URL(endpoint);
      expect(url.protocol).toBe('https:');
      expect(url.host).toBe('github.com');
      expect(url.pathname.startsWith('/trycua/cua/releases/download/')).toBe(true);
    }
  });

  it('verifies every update against a minisign public key', () => {
    const key = atob(updater.pubkey);
    expect(key).toContain('minisign public key');
  });
});
