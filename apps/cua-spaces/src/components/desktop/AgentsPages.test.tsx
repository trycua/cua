// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';

import { createAgentsBridgeOver } from '../../native/persistent';
import { AgentsPage, DRIVE_SYNC_POLL_MS, DrivePage, NOTIFICATIONS_SEEN_KEY, NotificationsPage, useAgentNotifications } from './AgentsPages';
import { renderHook } from '@testing-library/react';

const NOW = 1_790_000_000_000;

const MOUNTED = { enabled: true, state: 'mounted', method: 'fskit', path: '/Volumes/Cua Volume', volume_name: 'Cua Volume' };
const CONFLICT_COPY = 'public/plan (conflict from maya-linux 2026-09-29 14.02.11).md';

function syncStatus(over: Record<string, unknown> = {}) {
  return {
    device_id: 'd1',
    device_name: 'maya-mbp',
    feed: 'live',
    poll_interval_ms: 500,
    last_poll_ms: NOW - 3_000,
    last_remote_change_ms: NOW - 60_000,
    pending_uploads: 2,
    pending_bytes: 2048,
    conflicts: [
      { path: 'public/plan.md', conflict_path: CONFLICT_COPY, winner_device: 'd1', loser_device: 'd2', winner_version: 'v2', loser_version: 'v1', ts_ms: NOW - 120_000 },
    ],
    devices: [
      { id: 'd1', name: 'maya-mbp', this_device: true, last_seen_ms: NOW, last_change_ms: NOW, changes: 0 },
      { id: 'd2', name: 'maya-linux', this_device: false, last_seen_ms: NOW - 300_000, last_change_ms: 0, changes: 4 },
    ],
    last_error: null,
    ...over,
  };
}

/** An in-memory daemon: the tools the pages call, and what they were asked. */
function fakeDaemon(drive: { mount?: unknown; sync?: () => unknown; mountResult?: () => unknown } = {}) {
  const calls: string[] = [];
  const revealed: string[] = [];
  let paused = false;
  const routines = [{ id: 'R1', botID: 'ada', title: 'Morning', label: 'Every day at 8:00 AM', isEnabled: true }];
  let grants: { agent: string; machine: string; revoked: boolean }[] = [];
  let requests = [{ id: 'q1', principal: 'agent:ada', prefix: 'agents/bob/', mode: 'r', reason: 'cite' }];
  const feed = [{ id: 'n1', at_ms: NOW - 60_000, agent: 'ada', kind: 'turn_ended', title: 'ada', body: 'Your research is ready.', read: false }];
  const call = async (tool: string, args: Record<string, unknown> = {}) => {
    calls.push(`${tool} ${JSON.stringify(args)}`);
    switch (tool) {
      case 'persistent_agent_list':
        return { agents: [{ name: 'ada', harness: 'hermes', space: 'local:dev', paused, run_id: paused ? null : 'run-1', saved_ms: NOW - 120_000 }] };
      case 'agent_pause':
        paused = true;
        return {};
      case 'agent_resume':
        paused = false;
        return {};
      case 'volume_ls':
        return args.path === 'agents/ada/'
          ? { entries: [{ path: 'agents/ada/hermes/', name: 'hermes', folder: true }] }
          : args.path === 'agents/ada/hermes/'
            ? { entries: [{ path: 'agents/ada/hermes/MEMORY.md', name: 'MEMORY.md', folder: false, size: 19 }] }
            : { entries: [{ path: 'agents/', name: 'agents', folder: true }, { path: 'public/', name: 'public', folder: true }] };
      case 'volume_read':
        return { content: 'The user likes tea.', encoding: 'utf8' };
      case 'volume_history':
        return { versions: [{ version: 'v2', modified_ms: NOW, latest: true }, { version: 'v1', modified_ms: NOW - 3_600_000 }] };
      case 'routine_list':
        return { routines };
      case 'routine_add':
        routines.push({ id: 'R2', botID: 'ada', title: String(args.title), label: 'Every 30 minutes', isEnabled: true });
        return routines[1];
      case 'computer_access_list':
        return { grants, audit: [] };
      case 'computer_access_grant':
        grants = [{ agent: String(args.agent), machine: String(args.machine), revoked: false }];
        return grants[0];
      case 'volume_requests':
        return { requests };
      case 'volume_grants':
        return { grants: [] };
      case 'volume_approve':
        requests = [];
        return {};
      case 'notifications_list':
        return { notifications: feed };
      case 'notifications_ack':
        feed.forEach((n) => (n.read = true));
        return { marked: 1 };
      case 'volume_mount_status':
        if (drive.mount === undefined) throw new Error('unknown tool volume_mount_status');
        return drive.mount;
      case 'volume_sync_status':
        if (!drive.sync) throw new Error('unknown tool volume_sync_status');
        return drive.sync();
      case 'volume_sync_resolve':
        return null;
      case 'volume_mount':
        if (!drive.mountResult) throw new Error('unknown tool volume_mount');
        return drive.mountResult();
      default:
        throw new Error(`unknown tool ${tool}`);
    }
  };
  const reveal = async (path: string) => {
    revealed.push(path);
  };
  return { calls, revealed, bridge: createAgentsBridgeOver(call, reveal, async () => '/Users/maya') };
}

describe('AgentsPage', () => {
  it('pauses, opens memory, adds a routine and allows this computer', async () => {
    const { calls, bridge } = fakeDaemon();
    render(<AgentsPage bridge={bridge} thisMachine="relay:0123" now={() => NOW} />);
    expect(await screen.findByText('Hermes in local:dev, Running')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Pause' }));
    await waitFor(() => expect(screen.getByText('Hermes in local:dev, Paused')).toBeTruthy());
    expect(calls).toContain('agent_pause {"name":"ada"}');

    fireEvent.click(screen.getByRole('button', { name: 'ada' }));
    fireEvent.click(await screen.findByRole('button', { name: 'hermes/MEMORY.md' }));
    expect(await screen.findByText('The user likes tea.')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Restore' })).toBeTruthy();

    fireEvent.click(screen.getByRole('tab', { name: 'Routines' }));
    expect(screen.getByText('Morning')).toBeTruthy();
    fireEvent.change(screen.getByLabelText('Title'), { target: { value: 'Inbox sweep' } });
    fireEvent.change(screen.getByLabelText('Prompt'), { target: { value: 'Triage' } });
    fireEvent.click(screen.getByRole('button', { name: 'Add routine' }));
    await waitFor(() => expect(screen.getByText('Inbox sweep')).toBeTruthy());
    expect(calls.some((c) => c.startsWith('routine_add') && c.includes('"daily_at":"08:00"'))).toBe(true);

    fireEvent.click(screen.getByRole('tab', { name: 'Access to this computer' }));
    fireEvent.click(screen.getByRole('button', { name: 'Allow on this computer' }));
    await waitFor(() => expect(screen.getByText('This computer')).toBeTruthy());
    expect(calls).toContain('computer_access_grant {"agent":"ada","machine":"relay:0123"}');
  });
});

describe('DrivePage', () => {
  it('shows status, not files, and approves a request', async () => {
    const { calls, bridge } = fakeDaemon();
    render(<DrivePage bridge={bridge} />);
    expect(await screen.findByText('agent:ada asks to read agents/bob/')).toBeTruthy();
    // Finder is the file browser: no path bar, no listing, no volume_ls.
    expect(screen.queryByRole('navigation', { name: 'Path' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'agents' })).toBeNull();
    expect(calls.some((c) => c.startsWith('volume_ls'))).toBe(false);
    fireEvent.click(screen.getByRole('button', { name: 'Approve' }));
    await waitFor(() => expect(screen.queryByText('agent:ada asks to read agents/bob/')).toBeNull());
    expect(calls).toContain('volume_approve {"request_id":"q1"}');
  });
});

describe('DrivePage sync', () => {
  it('opens the mount in Finder, lists devices, and opens or resolves a conflict', async () => {
    let conflicts = true;
    const { calls, revealed, bridge } = fakeDaemon({
      mount: MOUNTED,
      sync: () => syncStatus(conflicts ? {} : { conflicts: [] }),
    });
    render(<DrivePage bridge={bridge} now={() => NOW} />);
    const open = await screen.findByRole('button', { name: 'Open in Finder' });
    expect(open.className).toContain('dw-btn-primary');
    expect(screen.getByText('In Finder at /Volumes/Cua Volume')).toBeTruthy();
    fireEvent.click(open);
    await waitFor(() => expect(revealed).toEqual(['/Volumes/Cua Volume']));
    expect(calls.some((c) => c.startsWith('volume_mount '))).toBe(false);

    const devices = screen.getByRole('region', { name: 'Devices' });
    expect(within(devices).getByText('maya-mbp (this device)')).toBeTruthy();
    expect(within(devices).getByText('2 pending, synced just now')).toBeTruthy();
    expect(within(devices).getByText('maya-linux')).toBeTruthy();
    expect(within(devices).getByText('Seen 5m ago')).toBeTruthy();

    const conflictsGroup = screen.getByRole('region', { name: 'Conflicts' });
    expect(within(conflictsGroup).getByText('public/plan.md')).toBeTruthy();
    expect(within(conflictsGroup).getByText('From maya-linux, 2m ago')).toBeTruthy();
    fireEvent.click(within(conflictsGroup).getByRole('button', { name: 'Open' }));
    await waitFor(() => expect(revealed).toEqual(['/Volumes/Cua Volume', `/Volumes/Cua Volume/${CONFLICT_COPY}`]));
    conflicts = false;
    fireEvent.click(within(conflictsGroup).getByRole('button', { name: 'Resolve' }));
    await waitFor(() => expect(screen.queryByRole('region', { name: 'Conflicts' })).toBeNull());
    expect(calls).toContain('volume_sync_resolve {"path":"public/plan.md"}');
  });

  it('leaves the mount and sync parts out when the daemon cannot answer them', async () => {
    const { calls, bridge } = fakeDaemon();
    render(<DrivePage bridge={bridge} now={() => NOW} />);
    expect(await screen.findByText('agent:ada asks to read agents/bob/')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Open in Finder' })).toBeNull();
    expect(screen.queryByRole('region', { name: 'Devices' })).toBeNull();
    expect(screen.queryByRole('region', { name: 'Conflicts' })).toBeNull();
    expect(calls.some((c) => c.startsWith('volume_mount_status'))).toBe(true);
    expect(calls.some((c) => c.startsWith('volume_sync_status'))).toBe(true);
  });

  it('shows a feed error as an error and one device on this Mac as a note', async () => {
    const { bridge, calls } = fakeDaemon({
      mount: { ...MOUNTED, state: 'off', path: null },
      sync: () => syncStatus({ feed: 'error', last_error: 'bucket unreachable', conflicts: [] }),
    });
    const { unmount } = render(<DrivePage bridge={bridge} now={() => NOW} />);
    expect(await screen.findByRole('alert')).toHaveTextContent('bucket unreachable');
    unmount();
    expect(calls.filter((c) => c.startsWith('volume_sync_status')).length).toBeGreaterThan(0);

    const off = fakeDaemon({ mount: MOUNTED, sync: () => syncStatus({ feed: 'off', conflicts: [] }) });
    render(<DrivePage bridge={off.bridge} now={() => NOW} />);
    const note = await screen.findByText('Syncing across devices needs S3-compatible storage.');
    expect(note.className).toBe('st-note');
    expect(screen.getByText('Not syncing')).toBeTruthy();
  });

  it('Open in Finder mounts first when the volume is not mounted, then reveals it', async () => {
    const home = { ...MOUNTED, path: '/Users/maya/Cua Volume' };
    let mounted = false;
    const { calls, revealed, bridge } = fakeDaemon({
      mount: { ...MOUNTED, state: 'off', path: null },
      sync: () => syncStatus({ conflicts: [] }),
      mountResult: () => {
        mounted = true;
        return home;
      },
    });
    render(<DrivePage bridge={bridge} now={() => NOW} />);
    expect(await screen.findByText('Not mounted')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Open in Finder' }));
    await waitFor(() => expect(revealed).toEqual(['/Users/maya/Cua Volume']));
    expect(mounted).toBe(true);
    expect(calls).toContain('volume_mount {}');
  });

  it('says why when mounting does not finish, and has no Open where it cannot mount', async () => {
    const { revealed, bridge } = fakeDaemon({
      mount: { ...MOUNTED, state: 'off', path: null },
      mountResult: () => ({ ...MOUNTED, state: 'needs_approval', path: null, detail: 'Turn on Cua Volume in File System Extensions.' }),
    });
    const { unmount } = render(<DrivePage bridge={bridge} now={() => NOW} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Open in Finder' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Turn on Cua Volume in File System Extensions.');
    expect(revealed).toEqual([]);
    unmount();

    const none = fakeDaemon({ mount: { enabled: false, state: 'unsupported', method: 'none', path: null } });
    render(<DrivePage bridge={none.bridge} now={() => NOW} />);
    expect(await screen.findByText('agent:ada asks to read agents/bob/')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Open in Finder' })).toBeNull();
  });

  it('reads the sync status again every 5 s while shown', async () => {
    vi.useFakeTimers();
    try {
      const { calls, bridge } = fakeDaemon({ mount: MOUNTED, sync: () => syncStatus() });
      const { unmount } = render(<DrivePage bridge={bridge} now={() => NOW} />);
      await vi.advanceTimersByTimeAsync(0);
      const reads = () => calls.filter((c) => c.startsWith('volume_sync_status')).length;
      expect(reads()).toBe(1);
      expect(DRIVE_SYNC_POLL_MS).toBe(5000);
      await vi.advanceTimersByTimeAsync(DRIVE_SYNC_POLL_MS);
      expect(reads()).toBe(2);
      unmount();
      await vi.advanceTimersByTimeAsync(DRIVE_SYNC_POLL_MS * 2);
      expect(reads()).toBe(2);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('notifications', () => {
  it('marks the backlog seen on first poll and posts a new entry once', async () => {
    const { bridge } = fakeDaemon();
    window.localStorage.setItem(NOTIFICATIONS_SEEN_KEY, '0');
    const post = vi.fn();
    const { result } = renderHook(() => useAgentNotifications(bridge, post, 60_000));
    await waitFor(() => expect(result.current.length).toBe(1));
    expect(post).not.toHaveBeenCalled();
    expect(Number(window.localStorage.getItem(NOTIFICATIONS_SEEN_KEY))).toBe(NOW - 60_000);
    render(<NotificationsPage feed={result.current} bridge={bridge} now={() => NOW} />);
    expect(screen.getByText('ada: Your research is ready.')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Mark all read' })).toBeTruthy();
  });
});
