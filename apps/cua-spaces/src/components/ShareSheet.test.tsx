// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { createFakeShareBridge } from '../native/share';
import { ShareSheet } from './ShareSheet';

const props = {
  spaceId: 'local:studio',
  spaceName: 'studio',
  signedIn: true,
  shareable: true,
  onClose: () => {},
};

describe('ShareSheet', () => {
  it('shares, changes a role and removes, one line per person', async () => {
    const bridge = createFakeShareBridge();
    render(<ShareSheet bridge={bridge} {...props} />);
    expect(await screen.findByText('Only you')).toBeTruthy();
    const field = screen.getByLabelText('Email or account ID');
    fireEvent.change(field, { target: { value: 'Bob@Example.com' } });
    fireEvent.change(screen.getByLabelText('Role'), { target: { value: 'editor' } });
    fireEvent.click(screen.getByRole('button', { name: 'Share' }));
    await waitFor(() => expect(screen.getByText('bob@example.com')).toBeTruthy());
    expect(bridge.calls).toContain('share:local:studio:bob@example.com:editor');
    fireEvent.change(screen.getByLabelText('Role of bob@example.com'), {
      target: { value: 'viewer' },
    });
    await waitFor(() =>
      expect(bridge.calls).toContain('share:local:studio:bob@example.com:viewer')
    );
    fireEvent.click(screen.getByRole('button', { name: 'Remove' }));
    await waitFor(() => expect(screen.getByText('Only you')).toBeTruthy());
    expect(bridge.calls).toContain('unshare:local:studio:bob@example.com');
  });

  it('shows a declined presence prompt and shares nothing', async () => {
    const bridge = createFakeShareBridge({ presenceFails: 'not shared: you declined' });
    render(<ShareSheet bridge={bridge} {...props} />);
    fireEvent.change(screen.getByLabelText('Email or account ID'), {
      target: { value: 'bob@example.com' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Share' }));
    expect((await screen.findByRole('alert')).textContent).toBe('not shared: you declined');
    expect(screen.getByText('Only you')).toBeTruthy();
  });

  it('explains why it cannot share when signed out', () => {
    render(<ShareSheet bridge={createFakeShareBridge()} {...props} signedIn={false} />);
    expect(screen.getByText('Sign in to cua.ai to share.')).toBeTruthy();
    expect((screen.getByRole('button', { name: 'Share' }) as HTMLButtonElement).disabled).toBe(
      true
    );
  });
});
