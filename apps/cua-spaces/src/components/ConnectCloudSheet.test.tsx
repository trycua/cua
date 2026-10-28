// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';

import type { CloudBridge, CloudTargetWire } from '../native/cloud';
import { ConnectCloudSheet } from './ConnectCloudSheet';

afterEach(cleanup);

function hetznerBridge() {
  const tests: CloudTargetWire[] = [];
  const connects: CloudTargetWire[] = [];
  const bridge: CloudBridge = {
    status: async () => ({
      providers: [
        { name: 'aws', title: 'AWS', tier: 'vm', connected: false, credentials: { found: true, source: '~/.aws profile default' } },
        { name: 'hetzner', title: 'Hetzner', tier: 'vm', connected: false, credentials: { found: false } },
      ],
    }),
    test: async (target) => {
      tests.push({ ...target });
      return { provider: target.provider, ok: true, account: 'p', checks: [{ name: 'credentials', ok: true }] };
    },
    connect: async (target) => {
      connects.push({ ...target });
      return { name: 'hetzner', title: 'Hetzner', tier: 'vm', connected: true, label: 'Hetzner · fsn1' };
    },
  };
  return { bridge, tests, connects };
}

describe('Connect a cloud: Hetzner', () => {
  it('asks for one masked API token, sends it on Test and Connect only, and drops it after', async () => {
    const { bridge, tests, connects } = hetznerBridge();
    const onConnected = vi.fn();
    render(<ConnectCloudSheet bridge={bridge} onConnected={onConnected} onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText('Hetzner')).toBeTruthy());
    expect(screen.queryByLabelText('API token')).toBeNull();

    fireEvent.click(screen.getByLabelText(/Hetzner/));
    const token = (await screen.findByLabelText('API token')) as HTMLInputElement;
    expect(token.type).toBe('password');
    expect((screen.getByRole('button', { name: 'Test' }) as HTMLButtonElement).disabled).toBe(true);

    const secret = 'demo-token-0123456789';
    fireEvent.change(token, { target: { value: secret } });
    await waitFor(() => expect((screen.getByRole('button', { name: 'Test' }) as HTMLButtonElement).disabled).toBe(false));
    fireEvent.click(screen.getByRole('button', { name: 'Test' }));
    await waitFor(() => expect(tests).toHaveLength(1));
    expect(tests[0]).toMatchObject({ provider: 'hetzner', token: secret });
    expect('uses_token' in tests[0]!).toBe(false);

    fireEvent.click(screen.getByRole('button', { name: 'Connect' }));
    await waitFor(() => expect(onConnected).toHaveBeenCalled());
    expect(connects[0]).toMatchObject({ provider: 'hetzner', token: secret });
    await waitFor(() => expect((screen.getByLabelText('API token') as HTMLInputElement).value).toBe(''));

    // Another provider never carries a token.
    fireEvent.click(screen.getByLabelText(/AWS/));
    expect(screen.queryByLabelText('API token')).toBeNull();
  });
});
