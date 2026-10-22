// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it } from 'vitest';

import { createFakeShareBridge } from '../native/share';
import { ShareRemovalBanner, ShareSheet } from './ShareSheet';

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

  it('explains that sharing is for teams, links to Teams and offers nothing, without a team', async () => {
    const opened: string[] = [];
    render(
      <ShareSheet
        bridge={createFakeShareBridge({ hasTeam: false })}
        {...props}
        onOpenExternal={(url) => opened.push(url)}
      />
    );
    expect(
      await screen.findByText('Sharing is for teams. Start a team to share this Space.')
    ).toBeTruthy();
    expect((screen.getByLabelText('Email or account ID') as HTMLInputElement).disabled).toBe(true);
    expect((screen.getByRole('button', { name: 'Share' }) as HTMLButtonElement).disabled).toBe(
      true
    );
    fireEvent.click(screen.getByRole('button', { name: 'See Teams' }));
    expect(opened).toEqual(['https://app.cua.ai/buy/teams?from=app_share']);
  });

  it('stays open in a team, and when it cannot tell (the server decides)', async () => {
    for (const hasTeam of [true, null]) {
      const { unmount } = render(
        <ShareSheet bridge={createFakeShareBridge({ hasTeam })} {...props} />
      );
      await screen.findByText('Only you');
      expect(screen.queryByRole('button', { name: 'See Teams' })).toBeNull();
      unmount();
    }
  });

  it('shows the server refusal in plain words: the sheet is not the only check', async () => {
    // The sheet lets the share through; the server says no.
    const bridge = createFakeShareBridge({ refuses: 'HTTP 403: share_outside_team', hasTeam: true });
    render(<ShareSheet bridge={bridge} {...props} />);
    fireEvent.change(screen.getByLabelText('Email or account ID'), {
      target: { value: 'alex@x.test' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Share' }));
    expect((await screen.findByRole('alert')).textContent).toBe(
      "alex@x.test isn't in your team. Invite them to your team first."
    );
    expect(bridge.calls).toContain('share:local:studio:alex@x.test:viewer');
  });
});

describe('ShareRemovalBanner', () => {
  it('names who was removed once, offers the invite and acknowledges when closed', async () => {
    const bridge = createFakeShareBridge({
      removals: [
        { who: 'alex@x.test', machineName: 'Design' },
        { who: 'sam@x.test', machineName: 'Design' },
      ],
    });
    const opened: string[] = [];
    render(<ShareRemovalBanner bridge={bridge} signedIn onOpenExternal={(u) => opened.push(u)} />);
    expect(
      await screen.findByText(
        'alex@x.test and sam@x.test can no longer open \u201cDesign\u201d, because they\'re not in your team.'
      )
    ).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Invite to your team' }));
    expect(opened).toEqual(['https://app.cua.ai/team']);
    fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    await waitFor(() => expect(bridge.calls).toContain('ackRemovals'));
    expect(screen.queryByRole('status')).toBeNull();
  });

  it('shows nothing when nobody was removed or signed out', async () => {
    const bridge = createFakeShareBridge();
    const { rerender } = render(<ShareRemovalBanner bridge={bridge} signedIn onOpenExternal={() => {}} />);
    await waitFor(() => expect(bridge.calls).toContain('removals'));
    expect(screen.queryByRole('status')).toBeNull();
    rerender(<ShareRemovalBanner bridge={createFakeShareBridge({ removals: [{ who: 'a@x.test', machineName: 'D' }] })} signedIn={false} onOpenExternal={() => {}} />);
    expect(screen.queryByRole('status')).toBeNull();
  });
});
