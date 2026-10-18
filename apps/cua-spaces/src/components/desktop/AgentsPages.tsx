// SPDX-License-Identifier: FSL-1.1-MIT
// Copyright (c) 2026 Cua AI, Inc.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import {
  agentsInitial,
  agentsView,
  driveInitial,
  driveView,
  notificationsPlan,
  notificationsView,
  reduceAgents,
  reduceDrive,
  type AgentsAction,
  type AgentsInput,
  type DriveAction,
  type DriveInput,
  type LineView,
  type NotificationInput,
} from '../../model/persistent';
import type { AgentsBridge } from '../../native/persistent';
import { readSetting, writeSetting } from '../../state/settings';
import { featureSignals } from "../../model/telemetry";
import { telemetryBridge } from "../../native/telemetry";

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** One grouped-list line: text, trailing text, up to two buttons, a switch. */
function Line({
  line,
  onAction,
  onSecondary,
  onToggle,
  onOpen,
}: {
  line: LineView;
  onAction?: () => void;
  onSecondary?: () => void;
  onToggle?: () => void;
  onOpen?: () => void;
}) {
  return (
    <div className="st-row" data-testid={`line-${line.id}`}>
      {onOpen ? (
        <button type="button" className="st-label ag-link" onClick={onOpen}>
          {line.text}
        </button>
      ) : (
        <span className="st-label">{line.text}</span>
      )}
      <span className="st-value">{line.trailing}</span>
      {line.on != null && onToggle && (
        <button type="button" role="switch" className="kv-switch" aria-checked={line.on} aria-label={line.text} onClick={onToggle} />
      )}
      {line.actionLabel && onAction && (
        <button type="button" className="dw-btn" onClick={onAction}>
          {line.actionLabel}
        </button>
      )}
      {line.secondaryLabel && onSecondary && (
        <button type="button" className="dw-btn" onClick={onSecondary}>
          {line.secondaryLabel}
        </button>
      )}
    </div>
  );
}

function Group({ title, children }: { title?: string; children: React.ReactNode }) {
  return (
    <section className="st-group" aria-label={title}>
      {title && (
        <div className="st-head">
          <h3 className="st-title">{title}</h3>
        </div>
      )}
      <div className="st-rows">{children}</div>
    </section>
  );
}

/**
 * "Agents": persistent agents, one line each, and the selected one's
 * memory, routines and access to this computer. The app core decides every
 * word; this loads its input and runs its requests through the daemon.
 */
export function AgentsPage({ bridge, thisMachine, now = Date.now }: { bridge: AgentsBridge; thisMachine: string | null; now?: () => number }) {
  const [input, setInput] = useState<AgentsInput>({ agents: [], thisMachine });
  // Persistent agents adoption: the page was opened.
  useEffect(() => telemetryBridge().recordSignals(featureSignals("agents_page_open")), []);
  const [state, setState] = useState(agentsInitial);
  const stateRef = useRef(state);
  stateRef.current = state;
  const v = agentsView(input, state, now());

  const reloadAgents = useCallback(async () => {
    const agents = await bridge.agents();
    setInput((i) => ({ ...i, agents }));
  }, [bridge]);

  useEffect(() => {
    void reloadAgents().catch(() => {});
  }, [reloadAgents]);

  const loadDetail = useCallback(
    async (name: string) => {
      const [home, routines, access] = await Promise.all([bridge.home(name), bridge.routines(name), bridge.access()]);
      setInput((i) => ({ ...i, home, routines, grants: access.grants, audit: access.audit }));
    },
    [bridge]
  );

  const send = useCallback(
    async (action: AgentsAction) => {
      const before = stateRef.current;
      const next = reduceAgents(input, before, action);
      setState(next);
      const request = next.request;
      if (!request || before.busy) return;
      try {
        if (request.kind === 'load') await loadDetail(request.name);
        else if (request.kind === 'read-file') {
          const file = await bridge.readFile(request.path);
          setInput((i) => ({ ...i, file }));
        } else {
          await bridge.run(request);
          if (request.kind === 'pause' || request.kind === 'resume') await reloadAgents();
          else if (next.selected) await loadDetail(next.selected);
        }
        setState((s) => reduceAgents(input, s, { type: 'done' }));
      } catch (e) {
        setState((s) => reduceAgents(input, s, { type: 'failed', error: message(e) }));
      }
    },
    [bridge, input, loadDetail, reloadAgents]
  );

  const d = v.detail;
  return (
    <div className="ag-page" aria-busy={v.busy}>
      <Group title={v.title}>
        {v.rows.length === 0 && <p className="st-note">{v.emptyText}</p>}
        {v.rows.map((r) => (
          <div className="st-row" key={r.name} data-selected={r.selected || undefined}>
            <button type="button" className="st-label ag-link" onClick={() => void send({ type: 'select', name: r.name })}>
              {r.name}
            </button>
            <span className="st-value">
              {r.detail}, {r.state}
            </span>
            <button
              type="button"
              className="dw-btn"
              onClick={() => void send(r.actionLabel === 'Resume' ? { type: 'resume', name: r.name } : { type: 'pause', name: r.name })}
            >
              {r.actionLabel}
            </button>
          </div>
        ))}
      </Group>
      {v.error && (
        <p className="st-error" role="alert">
          {v.error}
        </p>
      )}
      {d && (
        <>
          <div className="ag-detail-head">
            <h2>{d.name}</h2>
            <span className="st-note">{d.subtitle}</span>
          </div>
          <div className="ag-tabs" role="tablist">
            {d.tabs.map((t) => (
              <button key={t.tab} type="button" role="tab" aria-selected={t.selected} className="dw-btn" onClick={() => void send({ type: 'set-tab', tab: t.tab })}>
                {t.label}
              </button>
            ))}
          </div>
          {d.tabs.find((t) => t.selected)?.tab === 'memory' && (
            <>
              {d.file ? (
                <Group title={d.file.path}>
                  <pre className="ag-file">{d.file.text}</pre>
                  {d.file.versions.map((l) => (
                    <Line key={l.id} line={l} onAction={() => void send({ type: 'restore', version: l.id })} />
                  ))}
                  <div className="st-row">
                    <button type="button" className="dw-btn" onClick={() => void send({ type: 'close-file' })}>
                      {d.file.closeLabel}
                    </button>
                  </div>
                </Group>
              ) : (
                <Group>
                  {d.memory.length === 0 && <p className="st-note">{d.memoryEmpty}</p>}
                  {d.memory.map((l) => (
                    <Line key={l.id} line={l} onOpen={() => void send({ type: 'open-file', path: l.id })} />
                  ))}
                </Group>
              )}
            </>
          )}
          {d.tabs.find((t) => t.selected)?.tab === 'routines' && (
            <>
              <Group>
                {d.routines.length === 0 && <p className="st-note">{d.routinesEmpty}</p>}
                {d.routines.map((l) => (
                  <Line
                    key={l.id}
                    line={l}
                    onToggle={() => void send({ type: 'toggle-routine', id: l.id })}
                    onAction={() => void send({ type: 'remove-routine', id: l.id })}
                  />
                ))}
              </Group>
              <Group>
                <div className="st-row">
                  <input className="dw-input" aria-label="Title" placeholder="Title" value={d.form.title} onChange={(e) => void send({ type: 'set-title', title: e.target.value })} />
                </div>
                <div className="st-row">
                  <input className="dw-input" aria-label="Prompt" placeholder="Prompt" value={d.form.prompt} onChange={(e) => void send({ type: 'set-prompt', prompt: e.target.value })} />
                </div>
                <div className="st-row">
                  <select
                    className="dw-input"
                    aria-label="Schedule"
                    value={d.form.schedule}
                    onChange={(e) => void send({ type: 'set-schedule', schedule: e.target.value as 'every' | 'daily' | 'weekly' })}
                  >
                    {d.schedules.map((s) => (
                      <option key={s.id} value={s.id}>
                        {s.text}
                      </option>
                    ))}
                  </select>
                  {d.form.schedule === 'every' ? (
                    <input
                      className="dw-input"
                      aria-label="Minutes"
                      type="number"
                      min={1}
                      value={d.form.minutes}
                      onChange={(e) => void send({ type: 'set-minutes', minutes: Number(e.target.value) })}
                    />
                  ) : (
                    <input className="dw-input" aria-label="Time" value={d.form.time} onChange={(e) => void send({ type: 'set-time', time: e.target.value })} />
                  )}
                  {d.form.schedule === 'weekly' && (
                    <select className="dw-input" aria-label="Day" value={d.form.weekday} onChange={(e) => void send({ type: 'set-weekday', weekday: e.target.value })}>
                      {['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'].map((w) => (
                        <option key={w} value={w}>
                          {w}
                        </option>
                      ))}
                    </select>
                  )}
                  <button type="button" className="dw-btn dw-btn-primary" disabled={!d.canAddRoutine} onClick={() => void send({ type: 'add-routine' })}>
                    {d.addRoutineLabel}
                  </button>
                </div>
              </Group>
            </>
          )}
          {d.tabs.find((t) => t.selected)?.tab === 'access' && (
            <>
              <Group>
                {d.access.length === 0 && <p className="st-note">{d.accessEmpty}</p>}
                {d.access.map((l) => (
                  <Line key={l.id} line={l} onAction={() => void send({ type: 'revoke-computer', machine: l.id })} />
                ))}
                {d.allowThisMachineLabel && thisMachine && (
                  <div className="st-row">
                    <button type="button" className="dw-btn" onClick={() => void send({ type: 'allow-computer', machine: thisMachine })}>
                      {d.allowThisMachineLabel}
                    </button>
                  </div>
                )}
              </Group>
              {d.audit.length > 0 && (
                <Group>
                  {d.audit.map((l) => (
                    <Line key={l.id} line={l} />
                  ))}
                </Group>
              )}
            </>
          )}
        </>
      )}
    </div>
  );
}

/** How often the Drive page reads the sync status while shown. */
export const DRIVE_SYNC_POLL_MS = 5000;

/**
 * "Volume": the Cua Volume, Open in Finder while it is mounted, the devices
 * syncing it and files two of them wrote, requests waiting for the user,
 * and grants.
 */
export function DrivePage({ bridge, now = Date.now }: { bridge: AgentsBridge; now?: () => number }) {
  const [input, setInput] = useState<DriveInput>({ requests: [], grants: [], mount: null, sync: null, home: null });
  const [state, setState] = useState(driveInitial);
  const stateRef = useRef(state);
  stateRef.current = state;
  const v = driveView({ ...input, nowMs: now() }, state);

  const runRequest = useCallback(
    async (next: typeof state) => {
      const request = next.request;
      if (!request) return;
      try {
        await bridge.runDrive(request);
        // The requests and grants, and the mount and sync status (either
        // may be unavailable: the page then leaves those parts out).
        const [data, mount, sync, home] = await Promise.all([
          bridge.drive(),
          bridge.mountStatus(),
          bridge.syncStatus(),
          bridge.userHome(),
        ]);
        setInput({ ...data, mount, sync, home });
        setState((s) => reduceDrive(s, { type: 'done' }));
      } catch (e) {
        setState((s) => reduceDrive(s, { type: 'failed', error: message(e) }));
      }
    },
    [bridge]
  );

  useEffect(() => {
    void runRequest(stateRef.current);
  }, [runRequest]);

  useEffect(() => {
    const id = setInterval(() => {
      void bridge.syncStatus().then((sync) => setInput((i) => ({ ...i, sync })));
    }, DRIVE_SYNC_POLL_MS);
    return () => clearInterval(id);
  }, [bridge]);

  const send = useCallback(
    (action: DriveAction) => {
      const before = stateRef.current;
      const next = reduceDrive(before, action);
      setState(next);
      if (!before.busy && next.busy) void runRequest(next);
    },
    [runRequest]
  );

  return (
    <div className="ag-page" aria-busy={v.busy}>
      {v.openLabel && (
        <div className="ag-volume-head">
          <button
            type="button"
            className="dw-btn dw-btn-primary"
            disabled={v.busy}
            onClick={() => send({ type: 'open-volume', mounted: v.mountPath })}
          >
            {v.openLabel}
          </button>
          {v.mountLine && <span className="st-note">{v.mountLine}</span>}
        </div>
      )}
      {(v.devices.length > 0 || v.syncNote) && (
        <Group title={v.devicesTitle}>
          {v.devices.map((l) => (
            <Line key={l.id} line={l} />
          ))}
          {v.syncNote && (
            <p className={v.syncError ? 'st-error' : 'st-note'} role={v.syncError ? 'alert' : undefined}>
              {v.syncNote}
            </p>
          )}
        </Group>
      )}
      {v.conflicts.length > 0 && (
        <Group title={v.conflictsTitle}>
          {v.conflicts.map((c) => (
            <div className="st-row" key={c.path} data-testid={`conflict-${c.path}`}>
              <span className="st-label">{c.text}</span>
              <span className="st-value">{c.trailing}</span>
              {c.reveal && c.openLabel && (
                <button type="button" className="dw-btn" disabled={v.busy} onClick={() => send({ type: 'reveal', path: c.reveal! })}>
                  {c.openLabel}
                </button>
              )}
              <button type="button" className="dw-btn" disabled={v.busy} onClick={() => send({ type: 'resolve', path: c.path })}>
                {c.resolveLabel}
              </button>
            </div>
          ))}
        </Group>
      )}
      {v.requests.length > 0 && (
        <Group title={v.requestsTitle}>
          {v.requests.map((l) => (
            <Line key={l.id} line={l} onAction={() => send({ type: 'approve', id: l.id })} onSecondary={() => send({ type: 'deny', id: l.id })} />
          ))}
        </Group>
      )}
      <Group title={v.grantsTitle}>
        {v.grants.length === 0 && <p className="st-note">{v.grantsEmpty}</p>}
        {v.grants.map((l) => (
          <Line key={l.id} line={l} onAction={() => send({ type: 'revoke', id: l.id })} />
        ))}
      </Group>
      {v.error && (
        <p className="st-error" role="alert">
          {v.error}
        </p>
      )}
    </div>
  );
}

/** The last-seen marker in the app's settings file. */
export const NOTIFICATIONS_SEEN_KEY = 'cua.settings.notificationsSeenMs';

/**
 * Polls the daemon's feed while the app runs and posts what the app core
 * says to post (each entry once, none of the backlog on first run).
 */
export function useAgentNotifications(
  bridge: AgentsBridge,
  post: (title: string, body: string) => Promise<void> | void,
  everyMs = 5000
): NotificationInput[] {
  const [feed, setFeed] = useState<NotificationInput[]>([]);
  useEffect(() => {
    let stopped = false;
    const tick = async () => {
      try {
        const rows = await bridge.notifications();
        if (stopped) return;
        setFeed(rows);
        const seen = Number(readSetting(NOTIFICATIONS_SEEN_KEY, '0')) || 0;
        const plan = notificationsPlan(rows, seen);
        for (const n of plan.post) await post(n.title, n.body);
        if (plan.seenMs !== seen) writeSetting(NOTIFICATIONS_SEEN_KEY, String(plan.seenMs));
      } catch {
        // The daemon may be starting; try again next tick.
      }
    };
    void tick();
    const id = setInterval(() => void tick(), everyMs);
    return () => {
      stopped = true;
      clearInterval(id);
    };
  }, [bridge, post, everyMs]);
  return feed;
}

/** "Notifications": the feed, one line each, and Mark all read. */
export function NotificationsPage({
  feed,
  bridge,
  now = Date.now,
}: {
  feed: NotificationInput[];
  bridge: AgentsBridge;
  now?: () => number;
}) {
  const v = useMemo(() => notificationsView(feed, now()), [feed, now]);
  return (
    <div className="ag-page">
      <Group title={v.title}>
        {v.rows.length === 0 && <p className="st-note">{v.emptyText}</p>}
        {v.rows.map((l) => (
          <div className="st-row" key={l.id} data-unread={l.on || undefined}>
            <span className="st-label">{l.text}</span>
            <span className="st-value">{l.trailing}</span>
          </div>
        ))}
        {v.markAllLabel && (
          <div className="st-row">
            <button type="button" className="dw-btn" onClick={() => void bridge.markAllRead()}>
              {v.markAllLabel}
            </button>
          </div>
        )}
      </Group>
    </div>
  );
}
